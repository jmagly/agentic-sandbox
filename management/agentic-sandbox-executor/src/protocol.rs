//! A2A protocol-version negotiation and wire codecs.
//!
//! The executor persists one version-neutral task model and keeps the
//! protocol boundary here.  Legacy 0.3 clients continue to see the existing
//! `kind`-discriminated JSON while 1.0 clients use ProtoJSON enum values and
//! member-discriminated parts.

use axum::body::{to_bytes, Body};
use axum::http::header::{HeaderName, HeaderValue, CONTENT_TYPE, VARY};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_PROTOCOL_BODY_BYTES: usize = 4 * 1024 * 1024;
pub const A2A_VERSION_HEADER: HeaderName = HeaderName::from_static("a2a-version");
pub const A2A_MEDIA_TYPE: &str = "application/a2a+json";

static V03_REQUESTS: AtomicU64 = AtomicU64::new(0);
static V10_REQUESTS: AtomicU64 = AtomicU64::new(0);
static REJECTED_VERSION_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// Bounded-cardinality counters for the two supported versions and rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolMetricsSnapshot {
    pub v03_requests: u64,
    pub v10_requests: u64,
    pub rejected_version_requests: u64,
}

pub fn protocol_metrics_snapshot() -> ProtocolMetricsSnapshot {
    ProtocolMetricsSnapshot {
        v03_requests: V03_REQUESTS.load(Ordering::Relaxed),
        v10_requests: V10_REQUESTS.load(Ordering::Relaxed),
        rejected_version_requests: REJECTED_VERSION_REQUESTS.load(Ordering::Relaxed),
    }
}

/// Protocol semantics selected for a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolVersion {
    V0_3,
    V1_0,
}

impl ProtocolVersion {
    pub const fn as_header_value(self) -> &'static str {
        match self {
            Self::V0_3 => "0.3",
            Self::V1_0 => "1.0",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum NegotiationError {
    InvalidFormat(String),
    Unsupported(String),
    LegacyPath(String),
}

/// Resolve `A2A-Version`. Empty/missing means 0.3 as required by A2A 1.0.
///
/// Versions must use the exact Major.Minor form. Patch-formatted values are
/// rejected instead of being silently normalized.
pub fn negotiate(
    value: Option<&str>,
    legacy_path: bool,
) -> Result<ProtocolVersion, NegotiationError> {
    let value = value.unwrap_or("").trim();
    let selected = match value {
        "" | "0.3" => ProtocolVersion::V0_3,
        "1.0" => ProtocolVersion::V1_0,
        other if !is_major_minor(other) => {
            return Err(NegotiationError::InvalidFormat(other.to_string()));
        }
        other => return Err(NegotiationError::Unsupported(other.to_string())),
    };

    if legacy_path && selected != ProtocolVersion::V0_3 {
        return Err(NegotiationError::LegacyPath(value.to_string()));
    }
    Ok(selected)
}

fn is_major_minor(value: &str) -> bool {
    let Some((major, minor)) = value.split_once('.') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.bytes().all(|b| b.is_ascii_digit())
        && minor.bytes().all(|b| b.is_ascii_digit())
        && !minor.contains('.')
}

/// Convert a strict 1.0 request into the executor's legacy internal shape.
pub fn decode_v1_request(mut body: Value) -> Result<Value, String> {
    retain_known_fields(
        &mut body,
        &["tenant", "message", "configuration", "metadata"],
        "request",
    )?;
    if let Some(configuration) = body.get_mut("configuration") {
        retain_known_fields(
            configuration,
            &[
                "acceptedOutputModes",
                "taskPushNotificationConfig",
                "historyLength",
                "returnImmediately",
            ],
            "request.configuration",
        )?;
    }
    let message = body
        .get_mut("message")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "request.message must be an object".to_string())?;
    if message.contains_key("kind") {
        return Err("1.0 messages must not contain the legacy 'kind' discriminator".to_string());
    }
    decode_v1_message(message)?;
    Ok(body)
}

fn decode_v1_message(message: &mut Map<String, Value>) -> Result<(), String> {
    retain_known_map(
        message,
        &[
            "messageId",
            "contextId",
            "taskId",
            "role",
            "parts",
            "metadata",
            "extensions",
            "referenceTaskIds",
        ],
    );
    let message_id = message
        .get("messageId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "message.messageId is required".to_string())?;
    let _ = message_id;

    let role = message
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| "message.role is required".to_string())?;
    let legacy_role = match role {
        "ROLE_USER" => "user",
        "ROLE_AGENT" => "agent",
        other => return Err(format!("unsupported message.role '{other}'")),
    };
    message.insert("role".to_string(), Value::String(legacy_role.to_string()));

    let parts = message
        .get_mut("parts")
        .and_then(Value::as_array_mut)
        .filter(|parts| !parts.is_empty())
        .ok_or_else(|| "message.parts must contain at least one part".to_string())?;
    for (index, part) in parts.iter_mut().enumerate() {
        *part = decode_v1_part(part.take())
            .map_err(|error| format!("message.parts[{index}]: {error}"))?;
    }
    Ok(())
}

fn decode_v1_part(part: Value) -> Result<Value, String> {
    let mut object = part
        .as_object()
        .cloned()
        .ok_or_else(|| "part must be an object".to_string())?;
    if object.contains_key("kind") {
        return Err("1.0 parts must not contain the legacy 'kind' discriminator".to_string());
    }
    retain_known_map(
        &mut object,
        &[
            "text",
            "raw",
            "url",
            "data",
            "metadata",
            "filename",
            "mediaType",
        ],
    );

    let content_fields = ["text", "raw", "url", "data"];
    let populated: Vec<_> = content_fields
        .iter()
        .filter(|field| object.contains_key(**field))
        .copied()
        .collect();
    if populated.len() != 1 {
        return Err("part must contain exactly one of text, raw, url, or data".to_string());
    }
    match populated[0] {
        "text" | "url" => {
            if !object.get(populated[0]).is_some_and(Value::is_string) {
                return Err(format!("{} content must be a string", populated[0]));
            }
        }
        "raw" => {
            let raw = object
                .get("raw")
                .and_then(Value::as_str)
                .ok_or_else(|| "raw content must be a base64 string".to_string())?;
            base64::engine::general_purpose::STANDARD
                .decode(raw)
                .map_err(|_| "raw content must use valid base64".to_string())?;
        }
        "data" => {}
        _ => unreachable!(),
    }

    let media_type = object.remove("mediaType");
    let filename = object.remove("filename");
    let metadata = object.remove("metadata");
    let result = match populated[0] {
        "text" => {
            let mut result = json!({"kind": "text", "text": object.remove("text").unwrap()});
            copy_optional(&mut result, "metadata", metadata);
            copy_optional(&mut result, "mediaType", media_type);
            copy_optional(&mut result, "filename", filename);
            result
        }
        "data" => {
            let mut result = json!({"kind": "data", "data": object.remove("data").unwrap()});
            copy_optional(&mut result, "metadata", metadata);
            copy_optional(&mut result, "mediaType", media_type);
            copy_optional(&mut result, "filename", filename);
            result
        }
        field @ ("raw" | "url") => {
            let mut file = Map::new();
            let legacy_field = if field == "raw" { "bytes" } else { "uri" };
            file.insert(legacy_field.to_string(), object.remove(field).unwrap());
            if let Some(value) = media_type {
                file.insert("mimeType".to_string(), value);
            }
            if let Some(value) = filename {
                file.insert("name".to_string(), value);
            }
            let mut result = json!({"kind": "file", "file": Value::Object(file)});
            copy_optional(&mut result, "metadata", metadata);
            result
        }
        _ => unreachable!(),
    };
    Ok(result)
}

fn reject_unknown_fields(value: &Value, allowed: &[&str], context: &str) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    reject_unknown_map(object, allowed, context)
}

fn retain_known_fields(value: &mut Value, allowed: &[&str], context: &str) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| format!("{context} must be an object"))?;
    retain_known_map(object, allowed);
    Ok(())
}

fn retain_known_map(object: &mut Map<String, Value>, allowed: &[&str]) {
    object.retain(|field, _| allowed.contains(&field.as_str()));
}

fn reject_unknown_map(
    object: &Map<String, Value>,
    allowed: &[&str],
    context: &str,
) -> Result<(), String> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(format!("{context} contains unknown field '{field}'"));
    }
    Ok(())
}

fn copy_optional(target: &mut Value, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        target[key] = value;
    }
}

/// Convert a legacy response tree to A2A 1.0 ProtoJSON.
pub fn encode_v1_response(value: Value) -> Value {
    encode_value(value, None)
}

fn encode_value(value: Value, parent_key: Option<&str>) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| encode_value(value, parent_key))
                .collect(),
        ),
        Value::Object(mut object) => {
            if is_legacy_part(&object) {
                return encode_part(object);
            }

            if matches!(parent_key, Some("status")) {
                if let Some(Value::String(state)) = object.get_mut("state") {
                    *state = encode_state(state).to_string();
                }
                // These are executor persistence diagnostics, not TaskStatus fields.
                object.remove("fail_kind");
                object.remove("terminal_at");
                object.remove("error");
            }
            if let Some(Value::String(role)) = object.get_mut("role") {
                *role = match role.as_str() {
                    "user" => "ROLE_USER".to_string(),
                    "agent" => "ROLE_AGENT".to_string(),
                    other => other.to_string(),
                };
            }
            object.remove("kind");

            Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| {
                        let encoded = encode_value(value, Some(&key));
                        (protojson_field_name(&key).to_string(), encoded)
                    })
                    .collect(),
            )
        }
        Value::String(value) if parent_key == Some("timestamp") => Value::String(
            value
                .strip_suffix("+00:00")
                .map_or(value.clone(), |prefix| format!("{prefix}Z")),
        ),
        scalar => scalar,
    }
}

fn protojson_field_name(key: &str) -> &str {
    match key {
        "artifact_id" => "artifactId",
        "task_id" => "taskId",
        "created_at" => "createdAt",
        "next_cursor" => "nextPageToken",
        other => other,
    }
}

fn is_legacy_part(object: &Map<String, Value>) -> bool {
    matches!(
        object.get("kind").and_then(Value::as_str),
        Some("text" | "data" | "file")
    )
}

fn encode_part(mut object: Map<String, Value>) -> Value {
    let kind = object
        .remove("kind")
        .and_then(|value| value.as_str().map(str::to_string));
    match kind.as_deref() {
        Some("file") => {
            let file = object
                .remove("file")
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default();
            let mut encoded = Map::new();
            if let Some(value) = file.get("bytes").or_else(|| file.get("fileWithBytes")) {
                encoded.insert("raw".to_string(), value.clone());
            } else if let Some(value) = file.get("uri").or_else(|| file.get("fileWithUri")) {
                encoded.insert("url".to_string(), value.clone());
            }
            if let Some(value) = file.get("mimeType") {
                encoded.insert("mediaType".to_string(), value.clone());
            }
            if let Some(value) = file.get("name") {
                encoded.insert("filename".to_string(), value.clone());
            }
            if let Some(value) = object.remove("metadata") {
                encoded.insert(
                    "metadata".to_string(),
                    encode_value(value, Some("metadata")),
                );
            }
            Value::Object(encoded)
        }
        Some("text") | Some("data") => {
            let mut encoded = Map::new();
            for (key, value) in object {
                encoded.insert(key.clone(), encode_value(value, Some(&key)));
            }
            Value::Object(encoded)
        }
        _ => Value::Object(object),
    }
}

fn encode_state(state: &str) -> &str {
    match state {
        "submitted" => "TASK_STATE_SUBMITTED",
        "working" => "TASK_STATE_WORKING",
        "completed" => "TASK_STATE_COMPLETED",
        "failed" => "TASK_STATE_FAILED",
        "canceled" => "TASK_STATE_CANCELED",
        "rejected" => "TASK_STATE_REJECTED",
        "input-required" => "TASK_STATE_INPUT_REQUIRED",
        "auth-required" => "TASK_STATE_AUTH_REQUIRED",
        other => other,
    }
}

fn version_error(error: NegotiationError, response_media_type: &'static str) -> Response {
    let requested = match &error {
        NegotiationError::InvalidFormat(value)
        | NegotiationError::Unsupported(value)
        | NegotiationError::LegacyPath(value) => value.clone(),
    };
    let message = match error {
        NegotiationError::InvalidFormat(_) => {
            "A2A-Version must use the Major.Minor format".to_string()
        }
        NegotiationError::Unsupported(_) => format!(
            "A2A protocol version '{requested}' is not supported; supported versions are 0.3 and 1.0"
        ),
        NegotiationError::LegacyPath(_) => {
            "The /v1 compatibility path supports A2A 0.3 only; use the unprefixed interface for A2A 1.0".to_string()
        }
    };
    (
        StatusCode::BAD_REQUEST,
        [
            (CONTENT_TYPE, HeaderValue::from_static(response_media_type)),
            (A2A_VERSION_HEADER, HeaderValue::from_static("1.0")),
        ],
        json!({
            "error": {
                "code": 400,
                "status": "FAILED_PRECONDITION",
                "message": message,
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "VERSION_NOT_SUPPORTED",
                    "domain": "a2a-protocol.org",
                    "metadata": {
                        "requestedVersion": requested,
                        "supportedVersions": "0.3,1.0"
                    }
                }]
            }
        })
        .to_string(),
    )
        .into_response()
}

fn invalid_v1_request(message: String, response_media_type: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [
            (CONTENT_TYPE, HeaderValue::from_static(response_media_type)),
            (A2A_VERSION_HEADER, HeaderValue::from_static("1.0")),
        ],
        json!({
            "error": {
                "code": 400,
                "status": "INVALID_ARGUMENT",
                "message": message,
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "INVALID_REQUEST",
                    "domain": "a2a-protocol.org"
                }]
            }
        })
        .to_string(),
    )
        .into_response()
}

fn unsupported_content_type(content_type: &str, response_media_type: &'static str) -> Response {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        [
            (CONTENT_TYPE, HeaderValue::from_static(response_media_type)),
            (A2A_VERSION_HEADER, HeaderValue::from_static("1.0")),
        ],
        json!({
            "error": {
                "code": 415,
                "status": "UNIMPLEMENTED",
                "message": format!("Content-Type '{content_type}' is not supported"),
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                    "reason": "CONTENT_TYPE_NOT_SUPPORTED",
                    "domain": "a2a-protocol.org"
                }]
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Axum boundary that owns version selection and JSON transcoding.
pub async fn protocol_middleware(mut request: Request<Body>, next: Next) -> Response {
    let request_path = request.uri().path().to_string();
    let legacy_path = request.uri().path().split('/').any(|part| part == "v1");
    // v1.0.1 recommends application/a2a+json. Accept application/json for
    // v1.0/TCK compatibility and mirror it unless the client explicitly opts
    // into the corrected media type through Content-Type or Accept.
    let response_media_type = if request
        .headers()
        .get(CONTENT_TYPE)
        .or_else(|| request.headers().get(axum::http::header::ACCEPT))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim().starts_with(A2A_MEDIA_TYPE))
        }) {
        A2A_MEDIA_TYPE
    } else {
        "application/json"
    };
    let requested = request
        .headers()
        .get(&A2A_VERSION_HEADER)
        .and_then(|value| value.to_str().ok());
    let version = match negotiate(requested, legacy_path) {
        Ok(version) => version,
        Err(error) => {
            REJECTED_VERSION_REQUESTS.fetch_add(1, Ordering::Relaxed);
            return version_error(error, response_media_type);
        }
    };
    match version {
        ProtocolVersion::V0_3 => V03_REQUESTS.fetch_add(1, Ordering::Relaxed),
        ProtocolVersion::V1_0 => V10_REQUESTS.fetch_add(1, Ordering::Relaxed),
    };
    tracing::info!(
        a2a_protocol_version = version.as_header_value(),
        legacy_path,
        method = %request.method(),
        "selected A2A protocol version"
    );
    request.extensions_mut().insert(version);

    if version == ProtocolVersion::V1_0 && request.method() != axum::http::Method::GET {
        let (parts, body) = request.into_parts();
        let bytes = match to_bytes(body, MAX_PROTOCOL_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(error) => {
                return invalid_v1_request(
                    format!("request body could not be read: {error}"),
                    response_media_type,
                )
            }
        };
        if !bytes.is_empty() {
            let content_type = parts
                .headers
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            if !matches!(content_type, "application/json" | A2A_MEDIA_TYPE) {
                return unsupported_content_type(content_type, response_media_type);
            }
            let decoded = match serde_json::from_slice(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    if request_path.ends_with("/message:send")
                        || request_path.ends_with("/message:stream")
                        || request_path.ends_with("/messages:send")
                        || request_path.ends_with("/messages:stream")
                    {
                        decode_v1_request(value)
                    } else {
                        Ok(value)
                    }
                }) {
                Ok(value) => value,
                Err(error) => return invalid_v1_request(error, response_media_type),
            };
            request = Request::from_parts(parts, Body::from(decoded.to_string()));
            request.extensions_mut().insert(version);
        } else {
            request = Request::from_parts(parts, Body::empty());
            request.extensions_mut().insert(version);
        }
    }

    let mut response = next.run(request).await;
    response.headers_mut().insert(
        A2A_VERSION_HEADER,
        HeaderValue::from_static(version.as_header_value()),
    );
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("A2A-Version, Accept"));

    if version != ProtocolVersion::V1_0 {
        return response;
    }
    if let Some(location) = response
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
    {
        let canonical = location.replace("/v1/tasks/", "/tasks/");
        if let Ok(value) = HeaderValue::from_str(&canonical) {
            response
                .headers_mut()
                .insert(axum::http::header::LOCATION, value);
        }
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("text/event-stream") {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, MAX_PROTOCOL_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return invalid_v1_request(
                format!("response body could not be encoded: {error}"),
                response_media_type,
            )
        }
    };
    if bytes.is_empty() {
        parts
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static(response_media_type));
        return Response::from_parts(parts, Body::empty());
    }
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };
    let encoded = if parts.status.is_success() {
        let value = encode_v1_response(value);
        if (request_path.ends_with("/message:send") || request_path.ends_with("/messages:send"))
            && value.get("id").is_some()
            && value.get("task").is_none()
        {
            json!({"task": value})
        } else {
            value
        }
    } else {
        encode_v1_error(value, parts.status)
    };
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static(response_media_type));
    Response::from_parts(parts, Body::from(encoded.to_string()))
}

fn encode_v1_error(value: Value, status: StatusCode) -> Value {
    if value.get("error").is_some() {
        return value;
    }
    let message = value
        .get("detail")
        .or_else(|| value.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("A2A request failed");
    let legacy_code = value
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("request.failed");
    let reason = legacy_code
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    json!({
        "error": {
            "code": status.as_u16(),
            "status": canonical_status_name(status),
            "message": message,
            "details": [{
                "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                "reason": reason,
                "domain": "a2a-protocol.org"
            }]
        }
    })
}

fn canonical_status_name(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "INVALID_ARGUMENT",
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY => "FAILED_PRECONDITION",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::SERVICE_UNAVAILABLE => "UNAVAILABLE",
        StatusCode::BAD_GATEWAY => "INTERNAL",
        _ if status.is_server_error() => "INTERNAL",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_defaults_to_legacy_and_is_exact() {
        assert_eq!(negotiate(None, false), Ok(ProtocolVersion::V0_3));
        assert_eq!(negotiate(Some(""), false), Ok(ProtocolVersion::V0_3));
        assert_eq!(negotiate(Some("0.3"), false), Ok(ProtocolVersion::V0_3));
        assert!(matches!(
            negotiate(Some("0.3.0"), false),
            Err(NegotiationError::InvalidFormat(_))
        ));
        assert_eq!(negotiate(Some("1.0"), false), Ok(ProtocolVersion::V1_0));
        assert!(matches!(
            negotiate(Some("2.0"), false),
            Err(NegotiationError::Unsupported(_))
        ));
        assert!(matches!(
            negotiate(Some("1"), false),
            Err(NegotiationError::InvalidFormat(_))
        ));
        assert!(matches!(
            negotiate(Some("1.0"), true),
            Err(NegotiationError::LegacyPath(_))
        ));
    }

    #[test]
    fn v1_message_decodes_to_internal_shape() {
        let decoded = decode_v1_request(json!({
            "message": {
                "messageId": "m-1",
                "role": "ROLE_USER",
                "parts": [
                    {"text": "hello", "mediaType": "text/plain"},
                    {"url": "https://example.test/file", "filename": "f.txt", "mediaType": "text/plain"},
                    {"data": {"answer": 42}}
                ]
            }
        }))
        .unwrap();
        assert_eq!(decoded["message"]["role"], "user");
        assert_eq!(
            decoded["message"]["parts"][0],
            json!({"kind": "text", "text": "hello", "mediaType": "text/plain"})
        );
        assert_eq!(decoded["message"]["parts"][1]["kind"], "file");
        assert_eq!(
            decoded["message"]["parts"][1]["file"]["uri"],
            "https://example.test/file"
        );
        assert_eq!(decoded["message"]["parts"][2]["kind"], "data");
    }

    #[test]
    fn v1_rejects_mixed_or_legacy_parts() {
        for part in [
            json!({"kind": "text", "text": "legacy"}),
            json!({"text": "one", "data": {"two": true}}),
            json!({"mediaType": "text/plain"}),
        ] {
            assert!(decode_v1_request(json!({
                "message": {"messageId": "m-1", "role": "ROLE_USER", "parts": [part]}
            }))
            .is_err());
        }

        for invalid in [
            json!({
                "message": {"messageId": "m-1", "role": "ROLE_UNKNOWN", "parts": [{"text": "x"}]}
            }),
            json!({
                "message": {"messageId": "m-1", "role": "ROLE_USER", "parts": [{"raw": "not base64"}]}
            }),
            json!({
                "message": {"messageId": "m-1", "role": "ROLE_USER", "parts": [{"text": "x"}], "kind": "message"}
            }),
        ] {
            assert!(decode_v1_request(invalid).is_err());
        }
    }

    #[test]
    fn official_v101_style_fixture_round_trips_all_part_variants() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/a2a-v1.0.1-message.json"))
                .unwrap();
        let decoded = decode_v1_request(fixture).unwrap();
        let reencoded = encode_v1_response(decoded);

        assert_eq!(reencoded["message"]["role"], "ROLE_USER");
        assert_eq!(
            reencoded["message"]["parts"][0]["text"],
            "Analyze the attached inputs."
        );
        assert_eq!(reencoded["message"]["parts"][1]["raw"], "aGVsbG8=");
        assert_eq!(
            reencoded["message"]["parts"][2]["url"],
            "https://example.test/document.pdf"
        );
        assert_eq!(reencoded["message"]["parts"][3]["data"]["priority"], 3);
        assert_eq!(
            reencoded["message"]["extensions"],
            json!(["https://example.test/extensions/review/v1"])
        );
        assert_eq!(
            reencoded["message"]["metadata"]["fixtureSource"],
            "A2A-v1.0.1-ProtoJSON"
        );
    }

    #[test]
    fn response_encoder_applies_protojson_enums_and_parts() {
        let encoded = encode_v1_response(json!({
            "id": "t-1",
            "kind": "task",
            "status": {
                "state": "input-required",
                "fail_kind": "infrastructure",
                "terminal_at": "ignored"
            },
            "history": [{
                "kind": "message",
                "messageId": "m-1",
                "role": "agent",
                "parts": [
                    {"kind": "text", "text": "hello"},
                    {"kind": "file", "file": {"fileWithUri": "https://example.test/a", "mimeType": "image/png"}}
                ]
            }]
        }));
        assert!(encoded.get("kind").is_none());
        assert_eq!(encoded["status"]["state"], "TASK_STATE_INPUT_REQUIRED");
        assert!(encoded["status"].get("fail_kind").is_none());
        assert_eq!(encoded["history"][0]["role"], "ROLE_AGENT");
        assert!(encoded["history"][0]["parts"][0].get("kind").is_none());
        assert_eq!(
            encoded["history"][0]["parts"][1]["url"],
            "https://example.test/a"
        );
        assert_eq!(encoded["history"][0]["parts"][1]["mediaType"], "image/png");
    }

    #[test]
    fn response_encoder_normalizes_known_protojson_fields_and_utc_timestamps() {
        let encoded = encode_v1_response(json!({
            "tasks": [],
            "next_cursor": "cursor-1",
            "artifact_id": "artifact-1",
            "task_id": "task-1",
            "status": {
                "state": "working",
                "timestamp": "2026-08-29T18:00:00+00:00"
            }
        }));

        assert_eq!(encoded["nextPageToken"], "cursor-1");
        assert_eq!(encoded["artifactId"], "artifact-1");
        assert_eq!(encoded["taskId"], "task-1");
        assert_eq!(encoded["status"]["timestamp"], "2026-08-29T18:00:00Z");
    }
}

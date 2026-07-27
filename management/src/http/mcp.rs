//! Authenticated Streamable HTTP MCP management adapter.
//!
//! This is intentionally a small, stateless prototype: one `/mcp` endpoint
//! accepts JSON-RPC over POST and returns JSON responses. GET is authenticated
//! and returns 405 because the prototype does not emit unsolicited server
//! notifications. It implements the stable 2025-11-25 MCP lifecycle plus the
//! read-only tools and resources described by issue #656.

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use subtle::ConstantTimeEq;

use super::admin_v2;
use super::server::AppState;
use crate::output::{OutputMessage, StreamType};
use crate::session::TranscriptQuery;

const MCP_CONFIG_FILE: &str = "mcp-principals.toml";
const LATEST_STABLE_PROTOCOL: &str = "2025-11-25";
const SUPPORTED_PROTOCOLS: &[&str] = &["2025-03-26", "2025-06-18", LATEST_STABLE_PROTOCOL];
const DEFAULT_REPLAY_LIMIT: usize = 200;
const MAX_REPLAY_LIMIT: usize = 1000;

#[derive(Clone)]
struct McpPrincipal {
    client_id: String,
    token_hash: [u8; 32],
    scopes: HashSet<String>,
}

impl McpPrincipal {
    fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }
}

/// Hash-only bearer principal configuration for the MCP endpoint.
#[derive(Clone)]
pub struct McpConfig {
    principals: Vec<McpPrincipal>,
    allowed_origins: HashSet<String>,
}

#[derive(Debug, Deserialize)]
struct McpConfigFile {
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    allowed_origins: Vec<String>,
    #[serde(default)]
    principals: Vec<McpPrincipalFile>,
}

#[derive(Debug, Deserialize)]
struct McpPrincipalFile {
    client_id: String,
    token_hash: String,
    #[serde(default)]
    scopes: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

impl McpConfig {
    /// Load `<secrets_dir>/mcp-principals.toml`.
    ///
    /// A missing file or `enabled = false` leaves the endpoint disabled.
    /// Malformed or duplicate principal configuration fails startup instead
    /// of silently weakening authentication.
    pub fn load(secrets_dir: &Path) -> anyhow::Result<Option<Arc<Self>>> {
        let path = secrets_dir.join(MCP_CONFIG_FILE);
        if !path.exists() {
            tracing::info!(?path, "MCP principal config absent; /mcp disabled");
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let parsed: McpConfigFile = toml::from_str(&raw)?;
        if !parsed.enabled {
            tracing::info!(?path, "MCP principal config disabled; /mcp disabled");
            return Ok(None);
        }
        let config = Self::from_file(parsed)?;
        tracing::info!(
            ?path,
            principals = config.principals.len(),
            "loaded scoped MCP bearer principals"
        );
        Ok(Some(Arc::new(config)))
    }

    fn from_file(file: McpConfigFile) -> anyhow::Result<Self> {
        let mut client_ids = HashSet::new();
        let mut hashes = HashSet::new();
        let mut principals = Vec::with_capacity(file.principals.len());
        for principal in file.principals {
            let client_id = principal.client_id.trim().to_string();
            anyhow::ensure!(!client_id.is_empty(), "MCP client_id must not be empty");
            anyhow::ensure!(
                client_ids.insert(client_id.clone()),
                "duplicate MCP client_id: {client_id}"
            );
            let token_hash = parse_sha256_hash(&principal.token_hash)?;
            anyhow::ensure!(
                hashes.insert(token_hash),
                "duplicate MCP token_hash for client_id: {client_id}"
            );
            let scopes: HashSet<String> = principal
                .scopes
                .into_iter()
                .map(|scope| scope.trim().to_string())
                .filter(|scope| !scope.is_empty())
                .collect();
            principals.push(McpPrincipal {
                client_id,
                token_hash,
                scopes,
            });
        }
        anyhow::ensure!(
            !principals.is_empty(),
            "enabled MCP config requires at least one principal"
        );
        Ok(Self {
            principals,
            allowed_origins: file
                .allowed_origins
                .into_iter()
                .map(|origin| origin.trim().to_string())
                .filter(|origin| !origin.is_empty())
                .collect(),
        })
    }

    fn resolve(&self, token: &str) -> Option<McpPrincipal> {
        let presented: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        // Always compare every configured hash so a match's position does not
        // change how many constant-time comparisons the request performs.
        let mut resolved = None;
        for principal in &self.principals {
            if bool::from(principal.token_hash.ct_eq(&presented)) {
                resolved = Some(principal.clone());
            }
        }
        resolved
    }

    fn origin_allowed(&self, origin: Option<&str>) -> bool {
        match origin {
            None => true,
            Some(origin) => self.allowed_origins.contains(origin.trim()),
        }
    }

    #[cfg(test)]
    fn test_config(entries: &[(&str, &str, &[&str])]) -> Arc<Self> {
        Arc::new(Self {
            principals: entries
                .iter()
                .map(|(client_id, token, scopes)| McpPrincipal {
                    client_id: (*client_id).to_string(),
                    token_hash: Sha256::digest(token.as_bytes()).into(),
                    scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
                })
                .collect(),
            allowed_origins: HashSet::new(),
        })
    }
}

fn parse_sha256_hash(value: &str) -> anyhow::Result<[u8; 32]> {
    let value = value.trim().strip_prefix("sha256:").unwrap_or(value.trim());
    let bytes = hex::decode(value)?;
    anyhow::ensure!(bytes.len() == 32, "MCP token_hash must be SHA-256");
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

#[derive(Clone)]
struct McpState {
    app: AppState,
    config: Option<Arc<McpConfig>>,
}

/// Build the separately-authenticated `/mcp` route table.
pub fn router(app: AppState, config: Option<Arc<McpConfig>>) -> Router {
    Router::new()
        .route("/mcp", get(mcp_get).post(mcp_post))
        .with_state(McpState { app, config })
}

async fn mcp_get(State(state): State<McpState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize_connection(&state, &headers) {
        return response;
    }
    let mut response = (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({"error": "standalone MCP SSE stream is not enabled"})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}

async fn mcp_post(State(state): State<McpState>, headers: HeaderMap, body: Bytes) -> Response {
    let principal = match authorize_connection(&state, &headers) {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    if !is_json_content_type(&headers) {
        return transport_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        );
    }
    if !accepts_streamable_http(&headers) {
        return transport_error(
            StatusCode::NOT_ACCEPTABLE,
            "Accept must include application/json and text/event-stream",
        );
    }
    if let Some(version) = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
    {
        if !SUPPORTED_PROTOCOLS.contains(&version) {
            return transport_error(StatusCode::BAD_REQUEST, "unsupported MCP-Protocol-Version");
        }
    }

    let request: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(request)) => Value::Object(request),
        Ok(_) => {
            return rpc_response(
                StatusCode::BAD_REQUEST,
                rpc_error(Value::Null, -32600, "Invalid Request", None),
            )
        }
        Err(error) => {
            return rpc_response(
                StatusCode::BAD_REQUEST,
                rpc_error(
                    Value::Null,
                    -32700,
                    "Parse error",
                    Some(json!({"detail": error.to_string()})),
                ),
            )
        }
    };

    let method = match request.get("method").and_then(Value::as_str) {
        Some(method) if request.get("jsonrpc") == Some(&Value::String("2.0".to_string())) => method,
        _ => {
            return rpc_response(
                StatusCode::BAD_REQUEST,
                rpc_error(
                    request.get("id").cloned().unwrap_or(Value::Null),
                    -32600,
                    "Invalid Request",
                    None,
                ),
            )
        }
    };
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    tracing::debug!(
        client_id = %principal.client_id,
        method,
        "authenticated MCP request"
    );

    if method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = match id {
        Some(Value::String(id)) => Value::String(id),
        Some(Value::Number(id)) => Value::Number(id),
        _ => {
            return rpc_response(
                StatusCode::BAD_REQUEST,
                rpc_error(
                    Value::Null,
                    -32600,
                    "Request id must be a string or number",
                    None,
                ),
            )
        }
    };

    match dispatch(&state, &principal, method, params).await {
        Ok(result) => rpc_response(StatusCode::OK, rpc_result(id, result)),
        Err(error) => rpc_response(
            error.status,
            rpc_error(id, error.code, error.message, error.data),
        ),
    }
}

fn authorize_connection(state: &McpState, headers: &HeaderMap) -> Result<McpPrincipal, Response> {
    let config = state.config.as_ref().ok_or_else(|| {
        transport_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "MCP endpoint is not configured",
        )
    })?;
    let origin = match headers.get(header::ORIGIN) {
        Some(value) => match value.to_str() {
            Ok(origin) => Some(origin),
            Err(_) => {
                return Err(transport_error(
                    StatusCode::FORBIDDEN,
                    "Origin is not allowed",
                ))
            }
        },
        None => None,
    };
    if !config.origin_allowed(origin) {
        return Err(transport_error(
            StatusCode::FORBIDDEN,
            "Origin is not allowed",
        ));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case("bearer")
                .then_some(token.trim())
        })
        .filter(|token| !token.is_empty());
    match token.and_then(|token| config.resolve(token)) {
        Some(principal) => Ok(principal),
        None => {
            let mut response = transport_error(
                StatusCode::UNAUTHORIZED,
                "missing or invalid MCP bearer token",
            );
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"agentic-sandbox-mcp\""),
            );
            Err(response)
        }
    }
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
}

fn accepts_streamable_http(headers: &HeaderMap) -> bool {
    let Some(accept) = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mut accepts_json = false;
    let mut accepts_sse = false;
    for item in accept.split(',') {
        let media_type = item.split(';').next().unwrap_or_default().trim();
        accepts_json |= media_type.eq_ignore_ascii_case("application/json");
        accepts_sse |= media_type.eq_ignore_ascii_case("text/event-stream");
    }
    accepts_json && accepts_sse
}

struct DispatchError {
    status: StatusCode,
    code: i64,
    message: &'static str,
    data: Option<Value>,
}

impl DispatchError {
    fn invalid_params(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::OK,
            code: -32602,
            message: "Invalid params",
            data: Some(json!({"detail": detail.into()})),
        }
    }

    fn forbidden(scope: &str) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: -32003,
            message: "Insufficient scope",
            data: Some(json!({"required_scope": scope})),
        }
    }

    fn not_found(resource: &str) -> Self {
        Self {
            status: StatusCode::OK,
            code: -32002,
            message: "Resource not found",
            data: Some(json!({"resource": resource})),
        }
    }

    fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: -32000,
            message: "Management resource unavailable",
            data: Some(json!({"detail": detail.into()})),
        }
    }
}

async fn dispatch(
    state: &McpState,
    principal: &McpPrincipal,
    method: &str,
    params: Value,
) -> Result<Value, DispatchError> {
    match method {
        "initialize" => initialize(params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_definitions(principal)})),
        "tools/call" => call_tool(state, principal, params).await,
        "resources/list" => Ok(json!({"resources": resource_list(state, principal)})),
        "resources/templates/list" => {
            Ok(json!({"resourceTemplates": resource_templates(principal)}))
        }
        "resources/read" => read_resource(state, principal, params).await,
        _ => Err(DispatchError {
            status: StatusCode::OK,
            code: -32601,
            message: "Method not found",
            data: Some(json!({"method": method})),
        }),
    }
}

fn initialize(params: Value) -> Result<Value, DispatchError> {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::invalid_params("protocolVersion is required"))?;
    let negotiated = if SUPPORTED_PROTOCOLS.contains(&requested) {
        requested
    } else {
        LATEST_STABLE_PROTOCOL
    };
    Ok(json!({
        "protocolVersion": negotiated,
        "capabilities": {
            "tools": {"listChanged": false},
            "resources": {"subscribe": false, "listChanged": false}
        },
        "serverInfo": {
            "name": "agentic-sandbox-management",
            "title": "Agentic Sandbox Management",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Read-only sandbox fleet, session, and output management adapter."
    }))
}

fn tool_definitions(principal: &McpPrincipal) -> Vec<Value> {
    let mut tools = Vec::new();
    if principal.has_scope("fleet.read") {
        tools.push(json!({
            "name": "list_sandboxes",
            "title": "List sandboxes",
            "description": "List the canonical management fleet inventory.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "state": {"type": "string"},
                    "runtime": {"type": "string", "enum": ["qemu", "docker", "host"]},
                    "provider": {"type": "string"}
                },
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "required": ["items"],
                "properties": {"items": {"type": "array"}}
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false}
        }));
    }
    if principal.has_scope("output.read") {
        tools.push(json!({
            "name": "tail_output",
            "title": "Replay command output",
            "description": "Read bounded replay output for a command. Live follow is not part of this prototype.",
            "inputSchema": {
                "type": "object",
                "required": ["command_id"],
                "properties": {
                    "command_id": {"type": "string", "minLength": 1},
                    "agent_id": {"type": "string"},
                    "stream": {"type": "string", "enum": ["stdout", "stderr", "log"]},
                    "replay": {"type": "boolean", "default": true},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 1000},
                    "follow": {"type": "boolean", "default": false}
                },
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "required": ["command_id", "items", "replay"],
                "properties": {
                    "command_id": {"type": "string"},
                    "items": {"type": "array"},
                    "replay": {"type": "boolean"}
                }
            },
            "annotations": {"readOnlyHint": true, "destructiveHint": false}
        }));
    }
    tools
}

async fn call_tool(
    state: &McpState,
    principal: &McpPrincipal,
    params: Value,
) -> Result<Value, DispatchError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::invalid_params("tool name is required"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match name {
        "list_sandboxes" => {
            require_scope(principal, "fleet.read")?;
            let state_filter = optional_string(&arguments, "state")?;
            let runtime_filter = optional_string(&arguments, "runtime")?;
            if let Some(runtime) = runtime_filter {
                if !matches!(runtime, "qemu" | "docker" | "host") {
                    return Err(DispatchError::invalid_params(
                        "runtime must be qemu, docker, or host",
                    ));
                }
            }
            let provider_filter = optional_string(&arguments, "provider")?;
            let inventory = admin_v2::list_instances_value(
                &state.app,
                state_filter,
                runtime_filter,
                provider_filter,
            )
            .await;
            Ok(tool_result(inventory))
        }
        "tail_output" => {
            require_scope(principal, "output.read")?;
            let output = replay_output(&state.app, &arguments)?;
            Ok(tool_result(output))
        }
        _ => Err(DispatchError {
            status: StatusCode::OK,
            code: -32602,
            message: "Unknown tool",
            data: Some(json!({"name": name})),
        }),
    }
}

fn replay_output(app: &AppState, arguments: &Value) -> Result<Value, DispatchError> {
    let command_id = arguments
        .get("command_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DispatchError::invalid_params("command_id is required"))?;
    if arguments
        .get("follow")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(DispatchError::invalid_params(
            "follow=true is not supported by the replay-only prototype",
        ));
    }
    let replay = arguments
        .get("replay")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_REPLAY_LIMIT)
        .clamp(1, MAX_REPLAY_LIMIT);
    let agent_filter = optional_string(arguments, "agent_id")?;
    let stream_filter = match optional_string(arguments, "stream")? {
        None => None,
        Some("stdout") => Some(StreamType::Stdout),
        Some("stderr") => Some(StreamType::Stderr),
        Some("log") => Some(StreamType::Log),
        Some(_) => {
            return Err(DispatchError::invalid_params(
                "stream must be stdout, stderr, or log",
            ))
        }
    };

    let mut messages = if replay {
        app.output_agg.get_buffered(command_id)
    } else {
        Vec::new()
    };
    messages.retain(|message| {
        agent_filter.map_or(true, |agent_id| message.agent_id == agent_id)
            && stream_filter.map_or(true, |stream| message.stream_type == stream)
    });
    let start = messages.len().saturating_sub(limit);
    let items: Vec<Value> = messages[start..].iter().map(output_event).collect();
    Ok(json!({
        "command_id": command_id,
        "replay": replay,
        "items": items
    }))
}

fn output_event(message: &OutputMessage) -> Value {
    json!({
        "schema": "agentic.agent_output.v1",
        "event_type": "chunk",
        "agent_id": message.agent_id,
        "command_id": message.command_id,
        "stream": message.stream_type.to_string(),
        "timestamp_ms": message.timestamp,
        "data_base64": base64::engine::general_purpose::STANDARD.encode(&message.data),
        "text": String::from_utf8_lossy(&message.data)
    })
}

fn tool_result(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": value,
        "isError": false
    })
}

fn resource_list(state: &McpState, principal: &McpPrincipal) -> Vec<Value> {
    let mut resources = Vec::new();
    if principal.has_scope("fleet.read") {
        resources.push(resource(
            "sandbox://fleet",
            "fleet",
            "Current sandbox fleet inventory",
        ));
    }
    if principal.has_scope("session.read") {
        let agents: HashSet<String> = state
            .app
            .registry
            .list_agents()
            .into_iter()
            .map(|agent| agent.id)
            .collect();
        for agent_id in agents {
            resources.push(resource(
                &format!("sandbox://agents/{agent_id}/sessions"),
                &format!("{agent_id}-sessions"),
                "Sessions registered for this agent",
            ));
        }
        if let Some(registry) = state.app.session_registry.as_ref() {
            for session in registry.list() {
                resources.push(resource(
                    &format!("sandbox://sessions/{}/screen", session.session_id),
                    &format!("{}-screen", session.session_id),
                    "Current bounded terminal screen snapshot",
                ));
                resources.push(resource(
                    &format!("sandbox://sessions/{}/transcript", session.session_id),
                    &format!("{}-transcript", session.session_id),
                    "Bounded session transcript",
                ));
            }
        }
    }
    if principal.has_scope("output.read") {
        for command_id in state.app.output_agg.list_buffered_command_ids() {
            resources.push(resource(
                &format!("sandbox://commands/{command_id}/output"),
                &format!("{command_id}-output"),
                "Buffered command output",
            ));
        }
    }
    resources.sort_by(|a, b| {
        a.get("uri")
            .and_then(Value::as_str)
            .cmp(&b.get("uri").and_then(Value::as_str))
    });
    resources
}

fn resource(uri: &str, name: &str, description: &str) -> Value {
    json!({
        "uri": uri,
        "name": name,
        "description": description,
        "mimeType": "application/json"
    })
}

fn resource_templates(principal: &McpPrincipal) -> Vec<Value> {
    let mut templates = Vec::new();
    if principal.has_scope("fleet.read") {
        templates.push(json!({
            "uriTemplate": "sandbox://instances/{instance_id}",
            "name": "sandbox-instance",
            "description": "One sandbox instance from the canonical fleet inventory",
            "mimeType": "application/json"
        }));
    }
    if principal.has_scope("session.read") {
        templates.extend([
            json!({
                "uriTemplate": "sandbox://agents/{agent_id}/sessions",
                "name": "agent-sessions",
                "description": "Sessions for an agent",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sandbox://sessions/{session_id}/screen",
                "name": "session-screen",
                "description": "Current bounded terminal screen",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sandbox://sessions/{session_id}/transcript",
                "name": "session-transcript",
                "description": "Bounded transcript records",
                "mimeType": "application/json"
            }),
        ]);
    }
    if principal.has_scope("output.read") {
        templates.push(json!({
            "uriTemplate": "sandbox://commands/{command_id}/output",
            "name": "command-output",
            "description": "Buffered command output",
            "mimeType": "application/json"
        }));
    }
    templates
}

async fn read_resource(
    state: &McpState,
    principal: &McpPrincipal,
    params: Value,
) -> Result<Value, DispatchError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::invalid_params("resource uri is required"))?;
    let value = if uri == "sandbox://fleet" {
        require_scope(principal, "fleet.read")?;
        admin_v2::list_instances_value(&state.app, None, None, None).await
    } else if let Some(instance_id) = uri
        .strip_prefix("sandbox://instances/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
    {
        require_scope(principal, "fleet.read")?;
        let inventory = admin_v2::list_instances_value(&state.app, None, None, None).await;
        inventory
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(instance_id))
            })
            .cloned()
            .ok_or_else(|| DispatchError::not_found(uri))?
    } else if let Some(agent_id) = uri
        .strip_prefix("sandbox://agents/")
        .and_then(|rest| rest.strip_suffix("/sessions"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
    {
        require_scope(principal, "session.read")?;
        let sessions = state
            .app
            .session_registry
            .as_ref()
            .map(|registry| {
                registry
                    .list()
                    .into_iter()
                    .filter(|session| session.agent_id == agent_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        serde_json::to_value(sessions)
            .map_err(|error| DispatchError::unavailable(error.to_string()))?
    } else if let Some(session_id) = uri
        .strip_prefix("sandbox://sessions/")
        .and_then(|rest| rest.strip_suffix("/screen"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
    {
        require_scope(principal, "session.read")?;
        read_screen(&state.app, session_id)?
    } else if let Some(session_id) = uri
        .strip_prefix("sandbox://sessions/")
        .and_then(|rest| rest.strip_suffix("/transcript"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
    {
        require_scope(principal, "session.read")?;
        let registry = state
            .app
            .session_registry
            .as_ref()
            .ok_or_else(|| DispatchError::unavailable("session registry is not configured"))?;
        if !registry
            .list()
            .iter()
            .any(|session| session.session_id == session_id)
        {
            return Err(DispatchError::not_found(uri));
        }
        let records = registry
            .query_transcript(
                &session_id.to_string(),
                TranscriptQuery {
                    limit: 500,
                    ..TranscriptQuery::default()
                },
            )
            .await
            .map_err(|error| DispatchError::unavailable(error.to_string()))?;
        json!({"session_id": session_id, "items": records})
    } else if let Some(command_id) = uri
        .strip_prefix("sandbox://commands/")
        .and_then(|rest| rest.strip_suffix("/output"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
    {
        require_scope(principal, "output.read")?;
        replay_output(
            &state.app,
            &json!({"command_id": command_id, "replay": true, "limit": 1000}),
        )?
    } else {
        return Err(DispatchError::not_found(uri));
    };
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| DispatchError::unavailable(error.to_string()))?;
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": text
        }]
    }))
}

fn read_screen(app: &AppState, session_id: &str) -> Result<Value, DispatchError> {
    let sessions = app
        .session_registry
        .as_ref()
        .ok_or_else(|| DispatchError::unavailable("session registry is not configured"))?;
    let session = sessions
        .list()
        .into_iter()
        .find(|session| session.session_id == session_id)
        .ok_or_else(|| DispatchError::not_found(session_id))?;
    let screens = app
        .screen_registry
        .as_ref()
        .ok_or_else(|| DispatchError::unavailable("screen registry is not configured"))?;
    let screen = screens
        .get(&session.command_id)
        .ok_or_else(|| DispatchError::not_found(session_id))?;
    let snapshot = screen
        .lock()
        .map_err(|_| DispatchError::unavailable("screen state lock poisoned"))?
        .snapshot();
    Ok(json!({
        "session_id": session_id,
        "rows": snapshot.rows,
        "cols": snapshot.cols,
        "text": snapshot.text,
        "cursor_row": snapshot.cursor_row,
        "cursor_col": snapshot.cursor_col,
        "scrollback_tail": snapshot.scrollback_tail,
        "prompt": snapshot.prompt.map(|prompt| json!({
            "text": prompt.text,
            "confidence": prompt.confidence
        }))
    }))
}

fn require_scope(principal: &McpPrincipal, scope: &str) -> Result<(), DispatchError> {
    if principal.has_scope(scope) {
        Ok(())
    } else {
        Err(DispatchError::forbidden(scope))
    }
}

fn optional_string<'a>(value: &'a Value, field: &str) -> Result<Option<&'a str>, DispatchError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(DispatchError::invalid_params(format!(
            "{field} must be a string"
        ))),
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

fn rpc_response(status: StatusCode, payload: Value) -> Response {
    (status, Json(payload)).into_response()
}

fn transport_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::CommandDispatcher;
    use crate::output::OutputAggregator;
    use crate::registry::AgentRegistry;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_router(scopes: &[&str]) -> (Router, Arc<OutputAggregator>) {
        let registry = Arc::new(AgentRegistry::new());
        let output = Arc::new(OutputAggregator::default());
        let dispatcher = Arc::new(CommandDispatcher::new(registry.clone()));
        let app = AppState::new(registry, output.clone(), dispatcher);
        let config = McpConfig::test_config(&[("test-client", "test-token", scopes)]);
        (router(app, Some(config)), output)
    }

    fn request(body: Value, token: Option<&str>) -> axum::http::Request<axum::body::Body> {
        let mut builder = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    async fn response_json(response: Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn authentication_failure_is_unauthorized() {
        let (app, _) = test_router(&["fleet.read"]);
        let response = app
            .oneshot(request(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
    }

    #[tokio::test]
    async fn malformed_origin_is_forbidden() {
        let (app, _) = test_router(&["fleet.read"]);
        let mut request = request(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            Some("test-token"),
        );
        request.headers_mut().insert(
            header::ORIGIN,
            HeaderValue::from_bytes(b"https://admin.example/\xff").unwrap(),
        );
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn scope_denial_is_forbidden() {
        let (app, _) = test_router(&["fleet.read"]);
        let response = app
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":2,
                    "method":"tools/call",
                    "params":{"name":"tail_output","arguments":{"command_id":"cmd-1"}}
                }),
                Some("test-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let payload = response_json(response).await;
        assert_eq!(payload["error"]["data"]["required_scope"], "output.read");
    }

    #[tokio::test]
    async fn tools_list_is_scope_filtered_and_deterministic() {
        let (app, _) = test_router(&["fleet.read", "output.read"]);
        let response = app
            .oneshot(request(
                json!({"jsonrpc":"2.0","id":"tools","method":"tools/list"}),
                Some("test-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(payload["result"]["tools"][0]["name"], "list_sandboxes");
        assert_eq!(payload["result"]["tools"][1]["name"], "tail_output");
    }

    #[tokio::test]
    async fn resources_list_exposes_scoped_read_only_surfaces() {
        let (app, output) = test_router(&["fleet.read", "session.read", "output.read"]);
        output.push(
            "agent-1".to_string(),
            "cmd-1".to_string(),
            StreamType::Stdout,
            b"hello".to_vec(),
        );
        let response = app
            .oneshot(request(
                json!({"jsonrpc":"2.0","id":3,"method":"resources/list"}),
                Some("test-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        let uris: Vec<&str> = payload["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|resource| resource["uri"].as_str())
            .collect();
        assert!(uris.contains(&"sandbox://fleet"));
        assert!(uris.contains(&"sandbox://commands/cmd-1/output"));
    }

    #[tokio::test]
    async fn tail_output_maps_replayed_chunks_without_losing_bytes() {
        let (app, output) = test_router(&["output.read"]);
        output.push(
            "agent-1".to_string(),
            "cmd-1".to_string(),
            StreamType::Stdout,
            b"hello\xff".to_vec(),
        );
        let response = app
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":4,
                    "method":"tools/call",
                    "params":{
                        "name":"tail_output",
                        "arguments":{"command_id":"cmd-1","replay":true}
                    }
                }),
                Some("test-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        let item = &payload["result"]["structuredContent"]["items"][0];
        assert_eq!(item["agent_id"], "agent-1");
        assert_eq!(item["command_id"], "cmd-1");
        assert_eq!(item["stream"], "stdout");
        assert_eq!(item["data_base64"], "aGVsbG//");
        assert!(item["text"].as_str().unwrap().starts_with("hello"));
    }

    #[tokio::test]
    async fn initialize_negotiates_latest_stable_for_unknown_version() {
        let (app, _) = test_router(&["fleet.read"]);
        let response = app
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":5,
                    "method":"initialize",
                    "params":{
                        "protocolVersion":"2099-01-01",
                        "capabilities":{},
                        "clientInfo":{"name":"test","version":"1"}
                    }
                }),
                Some("test-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(payload["result"]["protocolVersion"], LATEST_STABLE_PROTOCOL);
    }

    #[tokio::test]
    async fn resources_read_returns_buffered_output() {
        let (app, output) = test_router(&["output.read"]);
        output.push(
            "agent-1".to_string(),
            "cmd-resource".to_string(),
            StreamType::Stderr,
            b"resource replay".to_vec(),
        );
        let response = app
            .oneshot(request(
                json!({
                    "jsonrpc":"2.0",
                    "id":6,
                    "method":"resources/read",
                    "params":{"uri":"sandbox://commands/cmd-resource/output"}
                }),
                Some("test-token"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = response_json(response).await;
        assert_eq!(
            payload["result"]["contents"][0]["uri"],
            "sandbox://commands/cmd-resource/output"
        );
        assert!(payload["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("resource replay"));
    }

    #[test]
    fn accept_media_types_must_match_exactly() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/jsonish, text/event-streaming"),
        );
        assert!(!accepts_streamable_http(&headers));
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json; charset=utf-8, text/event-stream; q=0.9"),
        );
        assert!(accepts_streamable_http(&headers));
    }
}

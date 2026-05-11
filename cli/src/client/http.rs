//! HTTP client for the management-server REST surface.
//!
//! - Built on reqwest, rustls, JSON.
//! - Attaches `Authorization: Bearer <token>` from the active context.
//! - Retries idempotent verbs (GET) once on 5xx; mutating verbs do not retry
//!   automatically — the caller decides because retrying a POST may double-act.
//! - Surfaces typed `ClientError` mapped to documented exit codes.

use std::time::Duration;
use thiserror::Error;

use crate::config::ContextEntry;

/// Documented exit codes for sandboxctl. Keep in sync with --help text.
pub const EXIT_OK: i32 = 0;
pub const EXIT_GENERIC: i32 = 1;
pub const EXIT_NOT_FOUND: i32 = 2;
pub const EXIT_CONFLICT: i32 = 3;
pub const EXIT_AUTH: i32 = 4;
pub const EXIT_TIMEOUT: i32 = 5;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("not found (404): {0}")]
    NotFound(String),
    #[error("conflict (409): {0}")]
    Conflict(String),
    #[error("auth required or denied ({status}): {body}")]
    Auth { status: u16, body: String },
    #[error("server error ({status}): {body}")]
    Server { status: u16, body: String },
    #[error("client error ({status}): {body}")]
    Client { status: u16, body: String },
    #[error("transport: {0}")]
    Transport(String),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("decode: {0}")]
    Decode(String),
}

impl ClientError {
    /// Map error to the documented exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            ClientError::NotFound(_) => EXIT_NOT_FOUND,
            ClientError::Conflict(_) => EXIT_CONFLICT,
            ClientError::Auth { .. } => EXIT_AUTH,
            ClientError::Timeout(_) => EXIT_TIMEOUT,
            _ => EXIT_GENERIC,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpClient {
    base: String,
    inner: reqwest::Client,
    token: String,
}

impl HttpClient {
    /// Base URL with no trailing slash. SSE/WS clients build their own
    /// URLs from this.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Reqwest client with the same TLS / pooling config as REST. SSE
    /// uses streaming responses and needs to bypass the JSON helper.
    pub fn inner_for_sse(&self) -> &reqwest::Client {
        &self.inner
    }

    /// Bearer token, if any. WebSocket clients may inject it as an
    /// `Authorization` header on the upgrade request.
    pub fn bearer_token(&self) -> Option<&str> {
        if self.token.is_empty() {
            None
        } else {
            Some(&self.token)
        }
    }

    pub fn new(ctx: &ContextEntry) -> Result<Self, ClientError> {
        let inner = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("sandboxctl/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        Ok(Self {
            base: ctx.server.trim_end_matches('/').to_string(),
            inner,
            token: ctx.token.clone(),
        })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base, path);
        let mut rb = self.inner.request(method, url);
        if !self.token.is_empty() {
            rb = rb.bearer_auth(&self.token);
        }
        rb
    }

    /// GET → JSON, retrying once on 5xx (idempotent).
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, ClientError> {
        let mut last_err: Option<ClientError> = None;
        for attempt in 0..2 {
            let res = self.req(reqwest::Method::GET, path).send().await;
            match res {
                Ok(r) => match handle(r).await {
                    Ok(body) => {
                        return serde_json::from_str::<T>(&body)
                            .map_err(|e| ClientError::Decode(e.to_string()))
                    }
                    Err(e @ ClientError::Server { .. }) if attempt == 0 => {
                        last_err = Some(e);
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                    Err(e) => return Err(e),
                },
                Err(e) if attempt == 0 && e.is_timeout() => {
                    last_err = Some(ClientError::Timeout(Duration::from_secs(30)));
                    continue;
                }
                Err(e) if e.is_timeout() => {
                    return Err(ClientError::Timeout(Duration::from_secs(30)))
                }
                Err(e) => return Err(ClientError::Transport(e.to_string())),
            }
        }
        Err(last_err.unwrap_or(ClientError::Transport("retry exhausted".into())))
    }

    /// GET → `serde_json::Value`. Convenience for verbs that pass the
    /// server response straight to the renderer without redeclaring shapes.
    pub async fn get_value(&self, path: &str) -> Result<serde_json::Value, ClientError> {
        self.get_json::<serde_json::Value>(path).await
    }

    /// GET → raw text (for non-JSON endpoints like `/metrics`).
    pub async fn get_text(&self, path: &str) -> Result<String, ClientError> {
        let r = self
            .req(reqwest::Method::GET, path)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        handle(r).await
    }

    /// POST with optional JSON body. Mutating: no retry.
    pub async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let mut rb = self.req(reqwest::Method::POST, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let r = rb
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let body = handle(r).await?;
        if body.is_empty() {
            // Allow callers expecting a unit-ish response to use serde_json::Value.
            serde_json::from_str::<T>("null").map_err(|e| ClientError::Decode(e.to_string()))
        } else {
            serde_json::from_str::<T>(&body).map_err(|e| ClientError::Decode(e.to_string()))
        }
    }

    /// POST raw bytes (no JSON envelope). Used by `storage push` —
    /// the server's `/api/v1/storage/{global,inbox/*}` accepts the
    /// file content as the request body directly.
    pub async fn post_bytes<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Vec<u8>,
    ) -> Result<T, ClientError> {
        let r = self
            .req(reqwest::Method::POST, path)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let body = handle(r).await?;
        if body.is_empty() {
            serde_json::from_str::<T>("null").map_err(|e| ClientError::Decode(e.to_string()))
        } else {
            serde_json::from_str::<T>(&body).map_err(|e| ClientError::Decode(e.to_string()))
        }
    }

    /// DELETE. Mutating: no retry.
    pub async fn delete_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, ClientError> {
        let r = self
            .req(reqwest::Method::DELETE, path)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let body = handle(r).await?;
        if body.is_empty() {
            serde_json::from_str::<T>("null").map_err(|e| ClientError::Decode(e.to_string()))
        } else {
            serde_json::from_str::<T>(&body).map_err(|e| ClientError::Decode(e.to_string()))
        }
    }

    /// DELETE with a JSON body. Mutating: no retry.
    /// Used by `task cancel --reason` (the `reason` ships in the body).
    pub async fn delete_with_body<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ClientError> {
        let r = self
            .req(reqwest::Method::DELETE, path)
            .json(body)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let body = handle(r).await?;
        if body.is_empty() {
            serde_json::from_str::<T>("null").map_err(|e| ClientError::Decode(e.to_string()))
        } else {
            serde_json::from_str::<T>(&body).map_err(|e| ClientError::Decode(e.to_string()))
        }
    }

    /// V2-first / V1-fallback dispatcher.
    ///
    /// Tries the v2 admin path first; on 404 falls back to v1 with a one-line
    /// `Sunset:` warning to stderr (printing the `Sunset` header from the v1
    /// response if present). Returns the response body as JSON plus a flag
    /// indicating which path served the response.
    ///
    /// `method` is upper-case (`"GET"`, `"POST"`, `"DELETE"`). `body` is an
    /// optional pre-serialized JSON object; for binary bodies use the
    /// non-fallback `post_bytes` directly.
    pub async fn try_v2_then_v1(
        &self,
        v2_path: &str,
        v1_path: &str,
        method: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(serde_json::Value, bool), ClientError> {
        let m = parse_method(method)?;
        // First attempt: v2.
        let v2_res = self.send_with_body(m.clone(), v2_path, body).await;
        match v2_res {
            Ok(v) => Ok((v, false)),
            Err(ClientError::NotFound(_)) => {
                // Fallback path. Probe the `Sunset` header on the v1 response
                // (informational only — we proceed regardless).
                let r = self
                    .req(m.clone(), v1_path);
                let mut rb = r;
                if let Some(b) = body {
                    rb = rb.json(b);
                }
                let resp = rb
                    .send()
                    .await
                    .map_err(|e| ClientError::Transport(e.to_string()))?;
                let sunset = resp
                    .headers()
                    .get("sunset")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let status = resp.status();
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ClientError::Transport(e.to_string()))?;
                if !status.is_success() {
                    let code = status.as_u16();
                    return Err(match code {
                        404 => ClientError::NotFound(text),
                        409 => ClientError::Conflict(text),
                        401 | 403 => ClientError::Auth {
                            status: code,
                            body: text,
                        },
                        500..=599 => ClientError::Server {
                            status: code,
                            body: text,
                        },
                        _ => ClientError::Client {
                            status: code,
                            body: text,
                        },
                    });
                }
                let summary = sunset
                    .as_deref()
                    .map(|s| format!(" (Sunset: {})", s))
                    .unwrap_or_default();
                eprintln!(
                    "warning: v2 admin path `{}` returned 404; falling back to v1 `{}`{}. \
                     v1 admin paths are scheduled for removal — please update to v2.",
                    v2_path, v1_path, summary
                );
                let v: serde_json::Value = if text.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str(&text)
                        .map_err(|e| ClientError::Decode(e.to_string()))?
                };
                Ok((v, true))
            }
            Err(e) => Err(e),
        }
    }

    /// Internal: send a request with an optional JSON body and return the
    /// decoded JSON value (or `Null` for empty bodies).
    async fn send_with_body(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, ClientError> {
        let mut rb = self.req(method, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let r = rb
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let body = handle(r).await?;
        if body.is_empty() {
            Ok(serde_json::Value::Null)
        } else {
            serde_json::from_str(&body).map_err(|e| ClientError::Decode(e.to_string()))
        }
    }
}

fn parse_method(m: &str) -> Result<reqwest::Method, ClientError> {
    match m.to_ascii_uppercase().as_str() {
        "GET" => Ok(reqwest::Method::GET),
        "POST" => Ok(reqwest::Method::POST),
        "PUT" => Ok(reqwest::Method::PUT),
        "DELETE" => Ok(reqwest::Method::DELETE),
        other => Err(ClientError::Transport(format!(
            "unsupported method `{}`",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContextEntry;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn make_client(url: &str) -> HttpClient {
        HttpClient::new(&ContextEntry {
            server: url.to_string(),
            token: "".into(),
            role: "operator".into(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn try_v2_then_v1_uses_v2_when_available() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v2/admin/instances"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items": []})))
            .mount(&server)
            .await;
        // Important: do NOT mount v1 — confirms v2 was hit first.
        let c = make_client(&server.uri());
        let (v, via_v1) = c
            .try_v2_then_v1("/api/v2/admin/instances", "/api/v1/agents", "GET", None)
            .await
            .expect("v2 ok");
        assert_eq!(via_v1, false);
        assert!(v.get("items").is_some());
    }

    #[tokio::test]
    async fn try_v2_then_v1_falls_back_to_v1_on_404_with_sunset_warning() {
        let server = MockServer::start().await;
        // v2 returns 404 → fallback engages.
        Mock::given(method("GET"))
            .and(path("/api/v2/admin/instances"))
            .respond_with(ResponseTemplate::new(404).set_body_string(""))
            .mount(&server)
            .await;
        // v1 returns 200 with a Sunset header.
        Mock::given(method("GET"))
            .and(path("/api/v1/agents"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Sunset", "Wed, 31 Dec 2026 23:59:59 GMT")
                    .set_body_json(serde_json::json!({"legacy": true})),
            )
            .mount(&server)
            .await;
        let c = make_client(&server.uri());
        let (v, via_v1) = c
            .try_v2_then_v1("/api/v2/admin/instances", "/api/v1/agents", "GET", None)
            .await
            .expect("fallback ok");
        assert_eq!(via_v1, true);
        assert_eq!(v["legacy"], serde_json::Value::Bool(true));
    }
}

async fn handle(r: reqwest::Response) -> Result<String, ClientError> {
    let status = r.status();
    let body = r
        .text()
        .await
        .map_err(|e| ClientError::Transport(e.to_string()))?;
    if status.is_success() {
        return Ok(body);
    }
    let code = status.as_u16();
    Err(match code {
        404 => ClientError::NotFound(body),
        409 => ClientError::Conflict(body),
        401 | 403 => ClientError::Auth { status: code, body },
        500..=599 => ClientError::Server { status: code, body },
        _ => ClientError::Client { status: code, body },
    })
}

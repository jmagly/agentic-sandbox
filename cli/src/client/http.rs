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
                Err(e) if e.is_timeout() => return Err(ClientError::Timeout(Duration::from_secs(30))),
                Err(e) => return Err(ClientError::Transport(e.to_string())),
            }
        }
        Err(last_err.unwrap_or(ClientError::Transport("retry exhausted".into())))
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
        let r = rb.send().await.map_err(|e| ClientError::Transport(e.to_string()))?;
        let body = handle(r).await?;
        if body.is_empty() {
            // Allow callers expecting a unit-ish response to use serde_json::Value.
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
}

async fn handle(r: reqwest::Response) -> Result<String, ClientError> {
    let status = r.status();
    let body = r.text().await.map_err(|e| ClientError::Transport(e.to_string()))?;
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

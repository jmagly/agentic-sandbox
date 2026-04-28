//! Operator authentication for the HTTP / WebSocket surface.
//!
//! Two-tier model:
//! - **Bearer token** (this module). Tokens live in a TOML file under
//!   `<secrets_dir>/operator-tokens.toml`. Each token is mapped to a role
//!   (`admin` or `operator`). Server boots with auth disabled if the file
//!   is missing — preserves the long-standing "trusted network" default.
//! - **Unix socket peer creds** (deferred — see issue #157 follow-up).
//!   Will resolve to `admin` automatically and skip token lookup.
//!
//! Wiring: `auth_middleware` runs as a router layer; if auth is enabled
//! it rejects unauthenticated requests with 401 and stashes the resolved
//! `OperatorRole` in request extensions. Destructive routes additionally
//! apply `require_admin` which returns 403 if the role isn't `Admin`.
//!
//! gRPC agent auth is independent and unchanged.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use parking_lot::RwLock;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

use super::server::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorRole {
    Admin,
    Operator,
}

impl OperatorRole {
    fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            _ => None,
        }
    }
}

/// Loaded auth configuration. `None` ⇒ auth disabled.
pub struct OperatorAuthConfig {
    /// SHA256(token-hex) → role. We only store hashes so a heap dump or
    /// log leak doesn't expose live bearer tokens.
    hashes: RwLock<HashMap<String, OperatorRole>>,
}

impl OperatorAuthConfig {
    /// Load from `<secrets_dir>/operator-tokens.toml`. Returns `Ok(None)`
    /// if the file does not exist (auth disabled). Returns `Err` only on
    /// malformed TOML or unreadable file — never silently disable auth.
    pub fn load(secrets_dir: &Path) -> anyhow::Result<Option<Arc<Self>>> {
        let path = secrets_dir.join("operator-tokens.toml");
        if !path.exists() {
            info!(
                "operator-tokens.toml not present at {:?}; HTTP/WS auth disabled",
                path
            );
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        let parsed: TokensFile = toml::from_str(&text)?;
        let mut hashes = HashMap::new();
        for entry in parsed.tokens {
            let role = match OperatorRole::from_str(&entry.role) {
                Some(r) => r,
                None => {
                    warn!("ignoring token with unknown role: {:?}", entry.role);
                    continue;
                }
            };
            hashes.insert(hash_token(&entry.token), role);
        }
        info!(
            count = hashes.len(),
            ?path,
            "loaded operator tokens; HTTP/WS auth enabled"
        );
        Ok(Some(Arc::new(Self {
            hashes: RwLock::new(hashes),
        })))
    }

    /// Resolve a presented bearer token to a role.
    pub fn resolve(&self, token: &str) -> Option<OperatorRole> {
        self.hashes.read().get(&hash_token(token)).copied()
    }
}

#[derive(Debug, Deserialize)]
struct TokensFile {
    #[serde(default)]
    tokens: Vec<TokenEntry>,
}

#[derive(Debug, Deserialize)]
struct TokenEntry {
    token: String,
    role: String,
}

fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

// ── Middleware ────────────────────────────────────────────────────────────

/// Bearer-token auth middleware. When `state.operator_auth` is `None`,
/// requests pass through unmodified (back-compat). When present, the
/// `Authorization: Bearer <token>` header is required; on success the
/// resolved `OperatorRole` is inserted into request extensions for
/// downstream handlers and `require_admin` to read.
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let cfg = match state.operator_auth.clone() {
        Some(c) => c,
        None => return next.run(req).await,
    };

    let header_val = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let token = match header_val.and_then(|s| s.strip_prefix("Bearer ")) {
        Some(t) => t.trim(),
        None => return unauthorized().into_response(),
    };
    let role = match cfg.resolve(token) {
        Some(r) => r,
        None => return unauthorized().into_response(),
    };
    req.extensions_mut().insert(role);
    next.run(req).await
}

/// Admin-only extractor. Add as a parameter on destructive handlers
/// (`_: RequireAdmin`) and the request will fail with 403 unless the
/// auth middleware resolved the caller to `OperatorRole::Admin`. When
/// auth is disabled (no `operator_auth` configured) the extractor
/// passes through — destructive verbs stay open in the "trusted network"
/// default until an operator-tokens.toml exists.
pub struct RequireAdmin;

impl<S> axum::extract::FromRequestParts<S> for RequireAdmin
where
    S: Send + Sync,
{
    type Rejection = Response;
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<OperatorRole>() {
            Some(OperatorRole::Admin) => Ok(RequireAdmin),
            Some(OperatorRole::Operator) => Err(forbidden().into_response()),
            // Auth disabled — let it through.
            None => Ok(RequireAdmin),
        }
    }
}

fn unauthorized() -> impl IntoResponse {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(serde_json::json!({"error": "missing or invalid bearer token"})),
    )
}

fn forbidden() -> impl IntoResponse {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "this verb requires the `admin` role"
        })),
    )
}

// Suppress unused-import warning when this module is included but no
// route uses Body directly.
const _: fn() = || {
    let _: Option<Body> = None;
};

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn missing_file_disables_auth() {
        let dir = tempdir().unwrap();
        assert!(OperatorAuthConfig::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn loaded_tokens_resolve_to_role() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("operator-tokens.toml");
        std::fs::write(
            &path,
            r#"
[[tokens]]
token = "alice-secret"
role = "admin"

[[tokens]]
token = "bob-secret"
role = "operator"
"#,
        )
        .unwrap();
        let cfg = OperatorAuthConfig::load(dir.path()).unwrap().unwrap();
        assert_eq!(cfg.resolve("alice-secret"), Some(OperatorRole::Admin));
        assert_eq!(cfg.resolve("bob-secret"), Some(OperatorRole::Operator));
        assert_eq!(cfg.resolve("eve-secret"), None);
    }

    #[test]
    fn unknown_role_skipped_without_failing_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("operator-tokens.toml");
        std::fs::write(
            &path,
            r#"
[[tokens]]
token = "ok"
role = "admin"

[[tokens]]
token = "bogus"
role = "superuser"
"#,
        )
        .unwrap();
        let cfg = OperatorAuthConfig::load(dir.path()).unwrap().unwrap();
        assert_eq!(cfg.resolve("ok"), Some(OperatorRole::Admin));
        assert_eq!(cfg.resolve("bogus"), None);
    }
}

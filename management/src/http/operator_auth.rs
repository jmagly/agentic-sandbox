//! Operator authentication for the HTTP / WebSocket surface.
//!
//! Three auth schemes are supported, tried in order by `RequireAdmin`:
//!
//! 1. **mTLS** — when the TLS listener is configured with client-auth
//!    required, the client certificate's subject CN is extracted and
//!    matched against `AIWG_MTLS_ADMIN_ALLOWLIST` (comma-separated CNs).
//!    A matching CN resolves to `OperatorRole::Admin`.
//! 2. **Unix peer-creds** — when the listener is a UNIX socket, the
//!    peer's UID is read via `SO_PEERCRED`. If `AIWG_UNIX_PEER_ADMIN_UID_ALLOWLIST`
//!    is set, the UID must appear there; otherwise (back-compat) any
//!    successful UDS connection grants `Admin` (filesystem ACL gate).
//! 3. **Bearer token** — tokens live in a TOML file under
//!    `<secrets_dir>/operator-tokens.toml`. Each token is mapped to a
//!    role (`admin` or `operator`). Server boots with bearer auth
//!    disabled if the file is missing — preserves the long-standing
//!    "trusted network" default.
//!
//! Wiring: `auth_middleware` runs as a router layer. mTLS and UDS
//! identities are pre-populated into request extensions by their
//! respective listener accept paths (see `tls_listener.rs`, `uds.rs`);
//! the middleware only handles the bearer path. `RequireAdmin` reads
//! `OperatorRole` from extensions — whoever populated it first wins.
//! Destructive routes apply `RequireAdmin` which returns 403 if the
//! role isn't `Admin`.
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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

use super::server::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorRole {
    Admin,
    Operator,
}

/// Authenticated operator identity resolved by the HTTP auth middleware.
/// Security-sensitive handlers should use this instead of trusting
/// caller-supplied actor fields in request bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorIdentity {
    pub actor: String,
    pub role: OperatorRole,
}

/// mTLS-derived identity, stashed in request extensions when the client
/// presented a verified certificate against a configured TLS listener
/// with client-auth required. The CN is the Subject Common Name from
/// the leaf certificate.
#[derive(Debug, Clone)]
pub struct MtlsIdentity {
    pub cn: String,
}

/// Unix-domain-socket peer credentials, stashed in request extensions
/// when the request arrived over a UDS listener. Read via SO_PEERCRED
/// at accept time.
#[derive(Debug, Clone, Copy)]
pub struct UnixPeerCreds {
    pub uid: u32,
    pub pid: Option<i32>,
}

/// mTLS auth policy. Loaded from `AIWG_MTLS_ADMIN_ALLOWLIST` (comma-
/// separated subject CNs). When the env var is unset or empty, mTLS
/// does NOT grant admin — even if the client presented a valid cert.
/// This is fail-closed: the operator must opt in by populating the
/// allowlist.
#[derive(Debug, Clone, Default)]
pub struct MtlsConfig {
    cns: Vec<String>,
}

impl MtlsConfig {
    /// Load from the environment. Returns an empty (no-grant) config
    /// when the env var is unset.
    pub fn from_env() -> Self {
        let raw = std::env::var("AIWG_MTLS_ADMIN_ALLOWLIST").unwrap_or_default();
        Self::from_csv(&raw)
    }

    pub fn from_csv(csv: &str) -> Self {
        let cns = csv
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        Self { cns }
    }

    /// Returns true if the CN should be granted the admin role.
    pub fn admits(&self, cn: &str) -> bool {
        self.cns.iter().any(|allowed| allowed == cn)
    }

    pub fn is_empty(&self) -> bool {
        self.cns.is_empty()
    }
}

/// Unix-peer-creds auth policy. Loaded from
/// `AIWG_UNIX_PEER_ADMIN_UID_ALLOWLIST` (comma-separated UIDs).
///
/// **Behavior matrix**:
/// - env var unset → back-compat: every UDS connection ⇒ `Admin`
///   (filesystem ACL on the socket path is the gate)
/// - env var set but empty (`""`) → fail-closed: no UID grants admin
/// - env var set with UIDs → only listed UIDs grant admin
#[derive(Debug, Clone, Default)]
pub struct UnixPeerCredsConfig {
    /// `Some(vec)` ⇒ allowlist active (may be empty for fail-closed).
    /// `None` ⇒ back-compat: grant admin to any UDS peer.
    uids: Option<Vec<u32>>,
}

impl UnixPeerCredsConfig {
    pub fn from_env() -> Self {
        match std::env::var("AIWG_UNIX_PEER_ADMIN_UID_ALLOWLIST") {
            Ok(raw) => Self::from_csv(&raw),
            Err(_) => Self {
                uids: None, // back-compat
            },
        }
    }

    pub fn from_csv(csv: &str) -> Self {
        let uids = csv
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<u32>().ok())
            .collect::<Vec<_>>();
        Self { uids: Some(uids) }
    }

    /// Construct with explicit back-compat behavior (admin for any UDS).
    pub fn back_compat() -> Self {
        Self { uids: None }
    }

    /// Returns true if the UID should be granted the admin role.
    pub fn admits(&self, uid: u32) -> bool {
        match &self.uids {
            None => true, // back-compat: any UDS peer is admin
            Some(list) => list.contains(&uid),
        }
    }

    /// True if a non-empty allowlist is configured (i.e., back-compat
    /// is disabled). Useful for `RequireAdmin` to know whether to
    /// trust an existing pre-populated `OperatorRole::Admin` extension
    /// from UDS accept, or whether to re-check the UID.
    pub fn is_explicit(&self) -> bool {
        self.uids.is_some()
    }
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
    /// Path to the source TOML; held so SIGHUP can re-parse the same file.
    source: PathBuf,
    /// Number of successful reloads since boot. Surfaced via /metrics.
    reload_count: AtomicU64,
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
        let hashes = Self::parse_file(&path)?;
        info!(
            count = hashes.len(),
            ?path,
            "loaded operator tokens; HTTP/WS auth enabled"
        );
        Ok(Some(Arc::new(Self {
            hashes: RwLock::new(hashes),
            source: path,
            reload_count: AtomicU64::new(0),
        })))
    }

    /// Re-parse the source TOML and atomically swap the token map.
    ///
    /// Atomic swap: build the new HashMap fully, then a single
    /// `RwLock::write` replaces it. There is no window during which
    /// both old and new tokens are active, and no window during which
    /// the map is empty.
    ///
    /// On parse / read error the previous map is kept intact and the
    /// caller gets the error — never silently disable auth.
    pub fn reload(&self) -> anyhow::Result<usize> {
        let new_hashes = Self::parse_file(&self.source)?;
        let count = new_hashes.len();
        *self.hashes.write() = new_hashes; // atomic swap
        let n = self.reload_count.fetch_add(1, Ordering::Relaxed) + 1;
        info!(
            count,
            reload_count = n,
            source = ?self.source,
            "operator-tokens.toml reloaded"
        );
        Ok(count)
    }

    fn parse_file(path: &Path) -> anyhow::Result<HashMap<String, OperatorRole>> {
        let text = std::fs::read_to_string(path)?;
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
        Ok(hashes)
    }

    /// Resolve a presented bearer token to a role.
    pub fn resolve(&self, token: &str) -> Option<OperatorRole> {
        self.hashes.read().get(&hash_token(token)).copied()
    }

    /// Number of currently-active tokens (for `/metrics`).
    pub fn active_count(&self) -> usize {
        self.hashes.read().len()
    }

    /// Number of successful reloads since boot (for `/metrics`).
    pub fn reload_count(&self) -> u64 {
        self.reload_count.load(Ordering::Relaxed)
    }
}

impl agentic_sandbox_executor::bindings::pty_ws::PtyAttachAuthorizer for OperatorAuthConfig {
    fn resolve_pty_scope(
        &self,
        token: &str,
    ) -> Option<agentic_sandbox_executor::bindings::pty_ws::PtyAttachScope> {
        match self.resolve(token)? {
            OperatorRole::Admin => {
                Some(agentic_sandbox_executor::bindings::pty_ws::PtyAttachScope::Admin)
            }
            OperatorRole::Operator => {
                Some(agentic_sandbox_executor::bindings::pty_ws::PtyAttachScope::Control)
            }
        }
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

/// The decision returned by `resolve_auth` for a single request. The
/// middleware (or a UDS listener) maps this into either an HTTP
/// response or an injected `OperatorRole` extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    /// Auth passed; grant this role. (For mTLS/UDS we only ever grant
    /// `Admin` — bearer can grant `Operator`.)
    Granted(OperatorRole),
    /// All schemes are off / no credentials presented AND bearer auth
    /// is disabled. Back-compat trusted-network pass-through.
    PassThrough,
    /// Bearer auth is enabled and no token / wrong token was presented.
    Unauthorized,
    /// mTLS cert presented but CN not in the allowlist.
    ForbiddenMtls(String),
    /// UDS peer-creds presented but UID not in the allowlist.
    ForbiddenUds(u32),
}

/// Pure-logic auth resolver. The middleware below is a thin wrapper
/// that performs the HTTP-level work (extension lookup, header parsing,
/// response building). All policy lives here so it can be tested
/// without an axum router.
pub fn resolve_auth(
    mtls_cfg: &MtlsConfig,
    unix_cfg: &UnixPeerCredsConfig,
    bearer_cfg: Option<&OperatorAuthConfig>,
    mtls_identity: Option<&MtlsIdentity>,
    unix_creds: Option<&UnixPeerCreds>,
    bearer_token: Option<&str>,
) -> AuthDecision {
    // 1. mTLS: presenting a cert is an explicit identity claim.
    //    If the CN matches the allowlist → admin. If it doesn't → 403
    //    (we don't fall through to bearer; that would defeat the cert
    //    restriction).
    if let Some(id) = mtls_identity {
        return if mtls_cfg.admits(&id.cn) {
            AuthDecision::Granted(OperatorRole::Admin)
        } else {
            AuthDecision::ForbiddenMtls(id.cn.clone())
        };
    }

    // 2. Unix peer-creds: arriving over UDS is also an explicit
    //    identity claim (filesystem ACL on the socket itself is the
    //    coarse gate; the UID allowlist is the fine-grained one).
    if let Some(creds) = unix_creds {
        return if unix_cfg.admits(creds.uid) {
            AuthDecision::Granted(OperatorRole::Admin)
        } else {
            AuthDecision::ForbiddenUds(creds.uid)
        };
    }

    // 3. Bearer token.
    let cfg = match bearer_cfg {
        Some(c) => c,
        None => return AuthDecision::PassThrough,
    };
    let token = match bearer_token {
        Some(t) => t.trim(),
        None => return AuthDecision::Unauthorized,
    };
    match cfg.resolve(token) {
        Some(role) => AuthDecision::Granted(role),
        None => AuthDecision::Unauthorized,
    }
}

/// Auth middleware. Tries the three schemes in order:
///
/// 1. **mTLS** — if a `MtlsIdentity` is in extensions (populated by the
///    TLS listener), resolve against `state.mtls_config` allowlist.
/// 2. **Unix peer-creds** — if `UnixPeerCreds` is in extensions
///    (populated by the UDS listener), resolve against
///    `state.unix_peer_creds_config` allowlist.
/// 3. **Bearer token** — if neither of the above resolved to a role
///    and `state.operator_auth` is configured, parse
///    `Authorization: Bearer <token>` and resolve against the token map.
///
/// When bearer auth is disabled (`operator_auth = None`) AND neither
/// mTLS nor UDS produced a role, the request passes through unmodified
/// (back-compat trusted-network default).
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    if is_unauthenticated_metadata_path(req.uri().path()) {
        return next.run(req).await;
    }

    // If an upstream listener already resolved a role (e.g., legacy UDS
    // back-compat path), trust it.
    if req.extensions().get::<OperatorRole>().is_some() {
        return next.run(req).await;
    }

    let mtls_id = req.extensions().get::<MtlsIdentity>().cloned();
    let unix_creds = req.extensions().get::<UnixPeerCreds>().copied();
    let bearer_token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let decision = resolve_auth(
        &state.mtls_config,
        &state.unix_peer_creds_config,
        state.operator_auth.as_deref(),
        mtls_id.as_ref(),
        unix_creds.as_ref(),
        bearer_token.as_deref(),
    );

    match decision {
        AuthDecision::Granted(role) => {
            req.extensions_mut().insert(role);
            if let Some(identity) = operator_identity_for_request(
                role,
                &state.mtls_config,
                &state.unix_peer_creds_config,
                state.operator_auth.as_deref(),
                mtls_id.as_ref(),
                unix_creds.as_ref(),
                bearer_token.as_deref(),
            ) {
                req.extensions_mut().insert(identity);
            }
            next.run(req).await
        }
        AuthDecision::PassThrough => next.run(req).await,
        AuthDecision::Unauthorized => unauthorized().into_response(),
        AuthDecision::ForbiddenMtls(cn) => forbidden_mtls(&cn).into_response(),
        AuthDecision::ForbiddenUds(uid) => forbidden_uds(uid).into_response(),
    }
}

fn operator_identity_for_request(
    role: OperatorRole,
    mtls_cfg: &MtlsConfig,
    unix_cfg: &UnixPeerCredsConfig,
    bearer_cfg: Option<&OperatorAuthConfig>,
    mtls_identity: Option<&MtlsIdentity>,
    unix_creds: Option<&UnixPeerCreds>,
    bearer_token: Option<&str>,
) -> Option<OperatorIdentity> {
    if let Some(id) = mtls_identity.filter(|id| mtls_cfg.admits(&id.cn)) {
        return Some(OperatorIdentity {
            actor: format!("mtls:{}", id.cn),
            role,
        });
    }
    if let Some(creds) = unix_creds.filter(|creds| unix_cfg.admits(creds.uid)) {
        return Some(OperatorIdentity {
            actor: format!("uid:{}", creds.uid),
            role,
        });
    }
    if let (Some(cfg), Some(token)) = (bearer_cfg, bearer_token) {
        if cfg.resolve(token.trim()).is_some() {
            return Some(OperatorIdentity {
                actor: format!("bearer:{}", role.as_str()),
                role,
            });
        }
    }
    None
}

impl OperatorRole {
    pub fn as_str(self) -> &'static str {
        match self {
            OperatorRole::Admin => "admin",
            OperatorRole::Operator => "operator",
        }
    }
}

fn is_unauthenticated_metadata_path(path: &str) -> bool {
    matches!(
        path,
        "/health"
            | "/healthz"
            | "/healthz/http"
            | "/healthz/libvirt"
            | "/readyz"
            | "/api/v1/bootstrap-enrollment/consume"
    ) || path.ends_with("/.well-known/agent-card.json")
        || path.ends_with("/.well-known/jwks.json")
        || path.ends_with("/v1/card")
        || path.ends_with("/v1/extendedAgentCard")
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

fn forbidden_mtls(cn: &str) -> impl IntoResponse {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "mTLS client certificate CN is not in the admin allowlist",
            "cn": cn,
        })),
    )
}

fn forbidden_uds(uid: u32) -> impl IntoResponse {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": "unix-socket peer UID is not in the admin allowlist",
            "uid": uid,
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
    use agentic_sandbox_executor::bindings::pty_ws::{PtyAttachAuthorizer, PtyAttachScope};
    use axum::extract::FromRequestParts;
    use axum::http::Request;
    use tempfile::tempdir;

    #[test]
    fn missing_file_disables_auth() {
        let dir = tempdir().unwrap();
        assert!(OperatorAuthConfig::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn metadata_paths_bypass_auth() {
        assert!(is_unauthenticated_metadata_path("/healthz"));
        assert!(is_unauthenticated_metadata_path("/health"));
        assert!(is_unauthenticated_metadata_path("/healthz/http"));
        assert!(is_unauthenticated_metadata_path("/healthz/libvirt"));
        assert!(is_unauthenticated_metadata_path("/readyz"));
        assert!(is_unauthenticated_metadata_path(
            "/api/v1/bootstrap-enrollment/consume"
        ));
        assert!(is_unauthenticated_metadata_path(
            "/agents/test/.well-known/agent-card.json"
        ));
        assert!(is_unauthenticated_metadata_path(
            "/agents/test/.well-known/jwks.json"
        ));
        assert!(is_unauthenticated_metadata_path("/agents/test/v1/card"));
        assert!(is_unauthenticated_metadata_path(
            "/agents/test/v1/extendedAgentCard"
        ));
        assert!(!is_unauthenticated_metadata_path("/healthz/deep"));
        assert!(!is_unauthenticated_metadata_path(
            "/agents/test/v1/tasks/task-id"
        ));
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
    fn reload_atomically_swaps_token_map() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("operator-tokens.toml");
        std::fs::write(
            &path,
            r#"
[[tokens]]
token = "v1-alice"
role = "admin"
"#,
        )
        .unwrap();
        let cfg = OperatorAuthConfig::load(dir.path()).unwrap().unwrap();
        assert_eq!(cfg.resolve("v1-alice"), Some(OperatorRole::Admin));
        assert_eq!(cfg.reload_count(), 0);

        // Replace the file with a different token set.
        std::fs::write(
            &path,
            r#"
[[tokens]]
token = "v2-bob"
role = "operator"
"#,
        )
        .unwrap();
        let n = cfg.reload().unwrap();
        assert_eq!(n, 1);
        assert_eq!(cfg.active_count(), 1);
        assert_eq!(cfg.reload_count(), 1);
        // Old token is gone, new token works.
        assert_eq!(cfg.resolve("v1-alice"), None);
        assert_eq!(cfg.resolve("v2-bob"), Some(OperatorRole::Operator));
    }

    #[test]
    fn reload_keeps_old_map_on_parse_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("operator-tokens.toml");
        std::fs::write(
            &path,
            r#"
[[tokens]]
token = "good"
role = "admin"
"#,
        )
        .unwrap();
        let cfg = OperatorAuthConfig::load(dir.path()).unwrap().unwrap();
        // Corrupt the file.
        std::fs::write(&path, "this is not valid toml [[[").unwrap();
        let res = cfg.reload();
        assert!(res.is_err(), "reload must fail on malformed TOML");
        // Previous tokens still active.
        assert_eq!(cfg.resolve("good"), Some(OperatorRole::Admin));
        assert_eq!(cfg.reload_count(), 0, "failed reload doesn't bump counter");
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

    // ── #238: mTLS + unix-peer-creds policy unit tests ────────────────────

    #[test]
    fn mtls_config_allowlist_csv_parses() {
        let cfg = MtlsConfig::from_csv("alice.example.com, bob.example.com , ");
        assert!(cfg.admits("alice.example.com"));
        assert!(cfg.admits("bob.example.com"));
        assert!(!cfg.admits("eve.example.com"));
        assert!(!cfg.admits(""));
        assert!(!cfg.is_empty());
    }

    #[test]
    fn mtls_config_empty_is_fail_closed() {
        let cfg = MtlsConfig::from_csv("");
        assert!(cfg.is_empty());
        assert!(!cfg.admits("alice"));
        assert!(!cfg.admits(""));
    }

    #[test]
    fn unix_peer_creds_config_back_compat_grants_any_uid() {
        let cfg = UnixPeerCredsConfig::back_compat();
        assert!(!cfg.is_explicit());
        // Any UID is admitted under back-compat (filesystem ACL gate).
        assert!(cfg.admits(0));
        assert!(cfg.admits(1000));
        assert!(cfg.admits(99999));
    }

    #[test]
    fn unix_peer_creds_config_allowlist_csv_parses() {
        let cfg = UnixPeerCredsConfig::from_csv("0, 1000 ,1001");
        assert!(cfg.is_explicit());
        assert!(cfg.admits(0));
        assert!(cfg.admits(1000));
        assert!(cfg.admits(1001));
        assert!(!cfg.admits(1002));
    }

    #[test]
    fn unix_peer_creds_config_empty_explicit_is_fail_closed() {
        // Env var set to "" means: allowlist exists, just empty.
        let cfg = UnixPeerCredsConfig::from_csv("");
        assert!(cfg.is_explicit());
        assert!(!cfg.admits(0));
        assert!(!cfg.admits(1000));
    }

    // ── resolve_auth: priority order and per-scheme outcomes ──────────────

    fn tokens_cfg() -> Arc<OperatorAuthConfig> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("operator-tokens.toml");
        std::fs::write(
            &path,
            r#"
[[tokens]]
token = "admin-tok"
role = "admin"
[[tokens]]
token = "op-tok"
role = "operator"
"#,
        )
        .unwrap();
        let cfg = OperatorAuthConfig::load(dir.path()).unwrap().unwrap();
        // Keep the temp dir alive by leaking — only the parsed hash map
        // matters for resolution.
        std::mem::forget(dir);
        cfg
    }

    #[test]
    fn bearer_auth_still_works() {
        let bearer = tokens_cfg();
        let decision = resolve_auth(
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            Some(&bearer),
            None,
            None,
            Some("admin-tok"),
        );
        assert_eq!(decision, AuthDecision::Granted(OperatorRole::Admin));

        let decision = resolve_auth(
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            Some(&bearer),
            None,
            None,
            Some("op-tok"),
        );
        assert_eq!(decision, AuthDecision::Granted(OperatorRole::Operator));

        let decision = resolve_auth(
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            Some(&bearer),
            None,
            None,
            Some("nope"),
        );
        assert_eq!(decision, AuthDecision::Unauthorized);

        let decision = resolve_auth(
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            Some(&bearer),
            None,
            None,
            None,
        );
        assert_eq!(decision, AuthDecision::Unauthorized);
    }

    #[test]
    fn operator_identity_tracks_resolved_auth_source() {
        let bearer = tokens_cfg();
        let identity = operator_identity_for_request(
            OperatorRole::Admin,
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            Some(&bearer),
            None,
            None,
            Some("admin-tok"),
        )
        .unwrap();
        assert_eq!(identity.actor, "bearer:admin");
        assert_eq!(identity.role, OperatorRole::Admin);

        let mtls = MtlsConfig::from_csv("operator.example.test");
        let identity = operator_identity_for_request(
            OperatorRole::Admin,
            &mtls,
            &UnixPeerCredsConfig::back_compat(),
            None,
            Some(&MtlsIdentity {
                cn: "operator.example.test".to_string(),
            }),
            None,
            None,
        )
        .unwrap();
        assert_eq!(identity.actor, "mtls:operator.example.test");

        let unix = UnixPeerCredsConfig::from_csv("1000");
        let identity = operator_identity_for_request(
            OperatorRole::Admin,
            &MtlsConfig::default(),
            &unix,
            None,
            None,
            Some(&UnixPeerCreds {
                uid: 1000,
                pid: Some(42),
            }),
            None,
        )
        .unwrap();
        assert_eq!(identity.actor, "uid:1000");
    }

    #[test]
    fn no_credentials_no_bearer_is_passthrough() {
        let decision = resolve_auth(
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            None,
            None,
            None,
            None,
        );
        assert_eq!(decision, AuthDecision::PassThrough);
    }

    #[test]
    fn mtls_admin_cn_in_allowlist_succeeds() {
        let mtls = MtlsConfig::from_csv("alice.example.com,bob.example.com");
        let id = MtlsIdentity {
            cn: "alice.example.com".into(),
        };
        let decision = resolve_auth(
            &mtls,
            &UnixPeerCredsConfig::back_compat(),
            None,
            Some(&id),
            None,
            None,
        );
        assert_eq!(decision, AuthDecision::Granted(OperatorRole::Admin));
    }

    #[test]
    fn mtls_admin_cn_not_in_allowlist_rejected() {
        let mtls = MtlsConfig::from_csv("alice.example.com");
        let id = MtlsIdentity {
            cn: "eve.example.com".into(),
        };
        let decision = resolve_auth(
            &mtls,
            &UnixPeerCredsConfig::back_compat(),
            None,
            Some(&id),
            None,
            None,
        );
        assert_eq!(
            decision,
            AuthDecision::ForbiddenMtls("eve.example.com".into())
        );
    }

    #[test]
    fn mtls_does_not_fall_through_to_bearer() {
        // Even if a valid bearer token is present, an mTLS identity
        // whose CN isn't in the allowlist must result in 403 — the
        // explicit identity claim wins.
        let bearer = tokens_cfg();
        let mtls = MtlsConfig::from_csv("alice.example.com");
        let id = MtlsIdentity {
            cn: "eve.example.com".into(),
        };
        let decision = resolve_auth(
            &mtls,
            &UnixPeerCredsConfig::back_compat(),
            Some(&bearer),
            Some(&id),
            None,
            Some("admin-tok"),
        );
        assert_eq!(
            decision,
            AuthDecision::ForbiddenMtls("eve.example.com".into())
        );
    }

    #[test]
    fn unix_peer_creds_uid_in_allowlist_succeeds() {
        let unix = UnixPeerCredsConfig::from_csv("1000,1001");
        let creds = UnixPeerCreds {
            uid: 1000,
            pid: Some(42),
        };
        let decision = resolve_auth(
            &MtlsConfig::default(),
            &unix,
            None,
            None,
            Some(&creds),
            None,
        );
        assert_eq!(decision, AuthDecision::Granted(OperatorRole::Admin));
    }

    #[test]
    fn unix_peer_creds_uid_not_in_allowlist_rejected() {
        let unix = UnixPeerCredsConfig::from_csv("1000,1001");
        let creds = UnixPeerCreds {
            uid: 9999,
            pid: Some(42),
        };
        let decision = resolve_auth(
            &MtlsConfig::default(),
            &unix,
            None,
            None,
            Some(&creds),
            None,
        );
        assert_eq!(decision, AuthDecision::ForbiddenUds(9999));
    }

    #[test]
    fn unix_peer_creds_back_compat_grants_any_uid() {
        // Default config (no env var) ⇒ any UDS peer is admin.
        let creds = UnixPeerCreds {
            uid: 31337,
            pid: None,
        };
        let decision = resolve_auth(
            &MtlsConfig::default(),
            &UnixPeerCredsConfig::back_compat(),
            None,
            None,
            Some(&creds),
            None,
        );
        assert_eq!(decision, AuthDecision::Granted(OperatorRole::Admin));
    }

    #[test]
    fn mtls_takes_priority_over_unix_peer_creds() {
        // If both are somehow present (test/synthetic), the mTLS
        // identity wins because cert-based identity is the strongest
        // claim.
        let mtls = MtlsConfig::from_csv("alice.example.com");
        let unix = UnixPeerCredsConfig::from_csv("1000");
        let id = MtlsIdentity {
            cn: "alice.example.com".into(),
        };
        let creds = UnixPeerCreds {
            uid: 1000,
            pid: None,
        };
        let decision = resolve_auth(&mtls, &unix, None, Some(&id), Some(&creds), None);
        assert_eq!(decision, AuthDecision::Granted(OperatorRole::Admin));
    }

    #[test]
    fn asvs_operator_auth_decision_matrix_covers_configured_modes() {
        let bearer = tokens_cfg();
        let mtls = MtlsConfig::from_csv("admin.example.test");
        let unix = UnixPeerCredsConfig::from_csv("1000");

        struct Case<'a> {
            label: &'a str,
            bearer_cfg: Option<&'a OperatorAuthConfig>,
            mtls_identity: Option<MtlsIdentity>,
            unix_creds: Option<UnixPeerCreds>,
            bearer_token: Option<&'a str>,
            expected: AuthDecision,
        }

        let cases = [
            Case {
                label: "auth disabled allows local compatibility pass-through",
                bearer_cfg: None,
                mtls_identity: None,
                unix_creds: None,
                bearer_token: None,
                expected: AuthDecision::PassThrough,
            },
            Case {
                label: "bearer auth enabled rejects missing token",
                bearer_cfg: Some(&bearer),
                mtls_identity: None,
                unix_creds: None,
                bearer_token: None,
                expected: AuthDecision::Unauthorized,
            },
            Case {
                label: "bearer auth enabled grants admin token",
                bearer_cfg: Some(&bearer),
                mtls_identity: None,
                unix_creds: None,
                bearer_token: Some("admin-tok"),
                expected: AuthDecision::Granted(OperatorRole::Admin),
            },
            Case {
                label: "bearer auth enabled grants operator token",
                bearer_cfg: Some(&bearer),
                mtls_identity: None,
                unix_creds: None,
                bearer_token: Some("op-tok"),
                expected: AuthDecision::Granted(OperatorRole::Operator),
            },
            Case {
                label: "mTLS allowlist grants admin",
                bearer_cfg: Some(&bearer),
                mtls_identity: Some(MtlsIdentity {
                    cn: "admin.example.test".into(),
                }),
                unix_creds: None,
                bearer_token: None,
                expected: AuthDecision::Granted(OperatorRole::Admin),
            },
            Case {
                label: "mTLS deny is fail-closed and does not fall through to bearer",
                bearer_cfg: Some(&bearer),
                mtls_identity: Some(MtlsIdentity {
                    cn: "intruder.example.test".into(),
                }),
                unix_creds: None,
                bearer_token: Some("admin-tok"),
                expected: AuthDecision::ForbiddenMtls("intruder.example.test".into()),
            },
            Case {
                label: "UDS allowlist grants admin",
                bearer_cfg: Some(&bearer),
                mtls_identity: None,
                unix_creds: Some(UnixPeerCreds {
                    uid: 1000,
                    pid: Some(7),
                }),
                bearer_token: None,
                expected: AuthDecision::Granted(OperatorRole::Admin),
            },
            Case {
                label: "UDS deny is fail-closed",
                bearer_cfg: Some(&bearer),
                mtls_identity: None,
                unix_creds: Some(UnixPeerCreds {
                    uid: 2000,
                    pid: Some(8),
                }),
                bearer_token: None,
                expected: AuthDecision::ForbiddenUds(2000),
            },
        ];

        for case in cases {
            let decision = resolve_auth(
                &mtls,
                &unix,
                case.bearer_cfg,
                case.mtls_identity.as_ref(),
                case.unix_creds.as_ref(),
                case.bearer_token,
            );
            assert_eq!(decision, case.expected, "{}", case.label);
        }
    }

    #[test]
    fn pty_attach_authorizer_maps_operator_roles_to_attach_scopes() {
        let bearer = tokens_cfg();

        assert_eq!(
            bearer.resolve_pty_scope("admin-tok"),
            Some(PtyAttachScope::Admin)
        );
        assert_eq!(
            bearer.resolve_pty_scope("op-tok"),
            Some(PtyAttachScope::Control)
        );
        assert_eq!(bearer.resolve_pty_scope("wrong-token"), None);
    }

    #[tokio::test]
    async fn require_admin_enforces_admin_role_when_auth_resolved() {
        let (mut parts, ()) = Request::builder()
            .uri("/sensitive")
            .body(())
            .unwrap()
            .into_parts();
        parts.extensions.insert(OperatorRole::Admin);
        assert!(RequireAdmin::from_request_parts(&mut parts, &())
            .await
            .is_ok());

        let (mut parts, ()) = Request::builder()
            .uri("/sensitive")
            .body(())
            .unwrap()
            .into_parts();
        parts.extensions.insert(OperatorRole::Operator);
        let rejection = match RequireAdmin::from_request_parts(&mut parts, &()).await {
            Ok(_) => panic!("operator role must not pass admin-only routes"),
            Err(rejection) => rejection,
        };
        assert_eq!(rejection.status(), StatusCode::FORBIDDEN);

        let (mut parts, ()) = Request::builder()
            .uri("/sensitive")
            .body(())
            .unwrap()
            .into_parts();
        assert!(RequireAdmin::from_request_parts(&mut parts, &())
            .await
            .is_ok());
    }
}

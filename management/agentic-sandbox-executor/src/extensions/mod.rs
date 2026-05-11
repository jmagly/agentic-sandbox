//! Server-side extension behaviors (#213).
//!
//! Each submodule implements one A2A extension advertised in the
//! AgentCard. The [`ExtensionRegistry`] runs registered handlers around
//! request handling: `pre_request` may short-circuit with a cached replay
//! or rejection; `post_response` may inject metadata into the outgoing
//! response body.
//!
//! ## Wire integration
//!
//! - Activated extensions arrive in the `A2A-Extensions` request header
//!   (comma-separated or repeated). [`ActivatedExtensions::from_headers`]
//!   parses both forms and lowercases for comparison.
//! - Required extensions ([`ExtensionHandler::required`] returns `true`)
//!   that are missing from the activated set produce a 400 error envelope
//!   via [`ExtensionRegistry::enforce_required`].
//! - The handler runs only when its URI is in the activated set; the
//!   registry calls [`ExtensionHandler::pre_request`] /
//!   [`ExtensionHandler::post_response`] for every registered handler
//!   and the handler itself checks activation. This keeps required
//!   handlers (runtime) running unconditionally as long as they are
//!   activated, and gating handlers (idempotency) opt in cleanly.
//!
//! ## Module map
//!
//! | Module | URI | Required | Behavior |
//! |---|---|---|---|
//! | [`runtime`] | `.../runtime/v1` | yes | Injects `runtime.*` into response metadata |
//! | [`hitl_prompt`] | `.../hitl-prompt/v1` | no | Validates HITL envelope shape on `input-required` |
//! | [`idempotency`] | `.../idempotency/v1` | no | Cache check (pre) + record (post) |
//! | [`multi_tenant`] | `.../multi-tenant/v1` | no | Reads `metadata.tenant_id`, records on span |
//! | [`pty_extensions`] | `.../pty-extensions/v1` | no | Stub; real wiring in W4.1 |

pub mod hitl_prompt;
pub mod idempotency;
pub mod multi_tenant;
pub mod pty_extensions;
pub mod runtime;

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::instance::RuntimeKind;
use crate::store::idempotency::IdempotencyCache;

// --- Activated extensions ---------------------------------------------------

/// Parsed `A2A-Extensions` request header — list of activated URIs.
///
/// Accepts either repeated header lines or a single comma-separated line
/// per A2A §3.4. Stored as-is (case-preserving) but [`Self::contains`]
/// matches case-insensitively per RFC 3986 §6.2.2.1 (URI scheme/host
/// case-insensitivity).
#[derive(Debug, Clone, Default)]
pub struct ActivatedExtensions(pub Vec<String>);

impl ActivatedExtensions {
    /// Build from request headers. Case-insensitive header lookup.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let uris: Vec<String> = headers
            .get_all("a2a-extensions")
            .iter()
            .flat_map(|v| v.to_str().ok())
            .flat_map(|s| s.split(','))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self(uris)
    }

    /// Returns `true` if `uri` is present (case-insensitive comparison
    /// for the scheme + authority; path is matched case-sensitively).
    pub fn contains(&self, uri: &str) -> bool {
        self.0.iter().any(|u| u.eq_ignore_ascii_case(uri))
    }

    /// Build a comma-separated `A2A-Extensions` response header value.
    pub fn to_response_header(&self) -> HeaderValue {
        let joined = self.0.join(", ");
        HeaderValue::from_str(&joined).unwrap_or_else(|_| HeaderValue::from_static(""))
    }

    /// Returns the underlying URI list.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

// --- Handler trait + contexts -----------------------------------------------

/// Pre-request hook context. Borrowed from the in-flight handler.
pub struct PreRequestCtx<'a> {
    pub activated: &'a ActivatedExtensions,
    pub task_id: Option<&'a str>,
    pub message_id: Option<&'a str>,
    pub request_body: &'a Value,
}

/// Post-response hook context. The handler may mutate `response_body`
/// in-place to inject extension-scoped metadata.
pub struct PostResponseCtx<'a> {
    pub activated: &'a ActivatedExtensions,
    pub task_id: &'a str,
    pub status: u16,
    pub response_body: &'a mut Value,
}

/// Outcome from [`ExtensionHandler::pre_request`].
#[derive(Debug)]
pub enum ExtensionOutcome {
    /// Continue to the next handler / main handler.
    Continue,
    /// Return the cached response (idempotency replay).
    Replay { status: u16, body: Value },
    /// Reject the request with an error envelope.
    Reject { status: u16, body: Value },
}

/// One server-side A2A extension behavior.
pub trait ExtensionHandler: Send + Sync {
    /// Stable URI identifying this extension.
    fn uri(&self) -> &'static str;

    /// Whether this extension is required on the AgentCard. Required
    /// extensions that are NOT activated produce a 400 error before any
    /// handler logic runs.
    fn required(&self) -> bool;

    /// Called before request handling. The default implementation is a
    /// no-op; handlers that need to short-circuit (e.g. idempotency
    /// replay) override this and return [`ExtensionOutcome::Replay`] or
    /// [`ExtensionOutcome::Reject`].
    fn pre_request(&self, _ctx: &PreRequestCtx<'_>) -> ExtensionOutcome {
        ExtensionOutcome::Continue
    }

    /// Called after request handling, before the response is sent.
    /// Handlers may mutate `ctx.response_body` to inject metadata.
    fn post_response(&self, _ctx: &mut PostResponseCtx<'_>) {}
}

// --- Registry ---------------------------------------------------------------

/// Registry of [`ExtensionHandler`]s. Cheaply cloneable via
/// `Arc<ExtensionRegistry>`; the handlers themselves live behind `Arc`.
#[derive(Default)]
pub struct ExtensionRegistry {
    handlers: Vec<Arc<dyn ExtensionHandler>>,
}

impl ExtensionRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Order matters: pre_request handlers are
    /// invoked in registration order; the first non-`Continue` outcome
    /// short-circuits.
    pub fn register(&mut self, handler: Arc<dyn ExtensionHandler>) {
        self.handlers.push(handler);
    }

    /// Run all handlers' `pre_request`. Returns the first non-`Continue`
    /// outcome, or `Continue` if all handlers continue.
    pub fn pre_request(&self, ctx: &PreRequestCtx<'_>) -> ExtensionOutcome {
        for h in &self.handlers {
            match h.pre_request(ctx) {
                ExtensionOutcome::Continue => continue,
                other => return other,
            }
        }
        ExtensionOutcome::Continue
    }

    /// Run all handlers' `post_response`. Each handler may mutate
    /// `ctx.response_body`.
    pub fn post_response(&self, ctx: &mut PostResponseCtx<'_>) {
        for h in &self.handlers {
            h.post_response(ctx);
        }
    }

    /// Verify all required extensions are present in the activated set.
    /// Returns `Err(error_body)` if any required extension is missing.
    /// The caller wraps in an HTTP 400 problem+json response.
    pub fn enforce_required(&self, activated: &ActivatedExtensions) -> Result<(), Value> {
        for h in &self.handlers {
            if h.required() && !activated.contains(h.uri()) {
                return Err(serde_json::json!({
                    "type": "https://agentic-sandbox.aiwg.io/errors/extension-required",
                    "title": "Required extension not activated",
                    "status": 400,
                    "code": "extension.required_not_activated",
                    "detail": format!(
                        "Required extension {} not in A2A-Extensions header",
                        h.uri()
                    ),
                }));
            }
        }
        Ok(())
    }

    /// Compute the activated set that should be echoed back to the
    /// client: the intersection of the request's activated set and the
    /// extensions actually registered on this executor.
    pub fn echo_activated(&self, activated: &ActivatedExtensions) -> ActivatedExtensions {
        let registered: Vec<String> = self.handlers.iter().map(|h| h.uri().to_string()).collect();
        let out: Vec<String> = activated
            .as_slice()
            .iter()
            .filter(|u| registered.iter().any(|r| r.eq_ignore_ascii_case(u)))
            .cloned()
            .collect();
        ActivatedExtensions(out)
    }

    /// Number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns `true` if no handlers are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

/// Build the default registry wired up with all v2.0 extensions.
///
/// - `runtime/v1` — required, injects runtime metadata.
/// - `hitl-prompt/v1` — declared, validates envelope shape.
/// - `idempotency/v1` — declared, hooks into [`IdempotencyCache`].
/// - `multi-tenant/v1` — declared-only in v2.0 (records on span).
/// - `pty-extensions/v1` — stub; real binding lands in W4.1.
pub fn build_default_registry(
    idem: Arc<IdempotencyCache>,
    runtime_kind: RuntimeKind,
    loadout: String,
    host: String,
) -> ExtensionRegistry {
    let mut r = ExtensionRegistry::new();
    r.register(Arc::new(runtime::RuntimeExtension::new(
        runtime_kind,
        loadout,
        host,
    )));
    r.register(Arc::new(hitl_prompt::HitlPromptExtension::new()));
    r.register(Arc::new(idempotency::IdempotencyExtension::new(idem)));
    r.register(Arc::new(multi_tenant::MultiTenantExtension::new()));
    r.register(Arc::new(pty_extensions::PtyExtension::new()));
    r
}

// --- Middleware: RequireA2AExtensions ---------------------------------------

/// Axum middleware that enforces `ExtensionRegistry::enforce_required`
/// against the request's `A2A-Extensions` header.
///
/// Applied via `axum::middleware::from_fn_with_state(registry, …)` to
/// mutating routes only (see `bindings::rest::router`). GET-only routes
/// bypass it so they remain reachable without negotiation.
///
/// On a missing required extension, returns 400 with the problem+json
/// envelope produced by `enforce_required`. The body shape matches
/// `docs/contracts/admin-api/error-envelope.schema.json` (the
/// `extension.required_not_activated` code is the contract-defined
/// machine code for this failure).
pub async fn require_extensions_middleware(
    State(registry): State<Arc<ExtensionRegistry>>,
    req: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let activated = ActivatedExtensions::from_headers(req.headers());
    if let Err(body) = registry.enforce_required(&activated) {
        return (
            StatusCode::BAD_REQUEST,
            [(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json"),
            )],
            body.to_string(),
        )
            .into_response();
    }
    next.run(req).await
}

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn mk_headers(vals: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in vals {
            h.append("A2A-Extensions", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn activated_extensions_parse_comma_separated() {
        let h = mk_headers(&["a, b, c"]);
        let a = ActivatedExtensions::from_headers(&h);
        assert_eq!(a.0, vec!["a", "b", "c"]);
    }

    #[test]
    fn activated_extensions_parse_repeated_headers() {
        let h = mk_headers(&["a", "b", "c"]);
        let a = ActivatedExtensions::from_headers(&h);
        assert_eq!(a.0, vec!["a", "b", "c"]);
    }

    #[test]
    fn activated_extensions_parse_case_insensitive_lookup() {
        let h = mk_headers(&["HTTPS://EXAMPLE/EXT/V1"]);
        let a = ActivatedExtensions::from_headers(&h);
        assert!(a.contains("https://example/ext/v1"));
    }

    #[test]
    fn activated_extensions_parse_trims_whitespace() {
        let h = mk_headers(&["  a  ,   b   "]);
        let a = ActivatedExtensions::from_headers(&h);
        assert_eq!(a.0, vec!["a", "b"]);
    }

    // --- Test handlers ------------------------------------------------------

    struct ContinueHandler {
        uri: &'static str,
        required: bool,
    }
    impl ExtensionHandler for ContinueHandler {
        fn uri(&self) -> &'static str {
            self.uri
        }
        fn required(&self) -> bool {
            self.required
        }
    }

    struct ReplayHandler {
        uri: &'static str,
    }
    impl ExtensionHandler for ReplayHandler {
        fn uri(&self) -> &'static str {
            self.uri
        }
        fn required(&self) -> bool {
            false
        }
        fn pre_request(&self, _ctx: &PreRequestCtx<'_>) -> ExtensionOutcome {
            ExtensionOutcome::Replay {
                status: 200,
                body: serde_json::json!({"replayed": true}),
            }
        }
    }

    struct RejectHandler {
        uri: &'static str,
    }
    impl ExtensionHandler for RejectHandler {
        fn uri(&self) -> &'static str {
            self.uri
        }
        fn required(&self) -> bool {
            false
        }
        fn pre_request(&self, _ctx: &PreRequestCtx<'_>) -> ExtensionOutcome {
            ExtensionOutcome::Reject {
                status: 422,
                body: serde_json::json!({"rejected": true}),
            }
        }
    }

    #[test]
    fn registry_pre_request_first_non_continue_wins() {
        let mut r = ExtensionRegistry::new();
        r.register(Arc::new(ContinueHandler {
            uri: "u1",
            required: false,
        }));
        r.register(Arc::new(ReplayHandler { uri: "u2" }));
        r.register(Arc::new(RejectHandler { uri: "u3" }));

        let activated = ActivatedExtensions::default();
        let body = Value::Null;
        let ctx = PreRequestCtx {
            activated: &activated,
            task_id: None,
            message_id: None,
            request_body: &body,
        };
        match r.pre_request(&ctx) {
            ExtensionOutcome::Replay { status, .. } => assert_eq!(status, 200),
            other => panic!("expected Replay, got {other:?}"),
        }
    }

    #[test]
    fn enforce_required_when_missing_returns_error() {
        let mut r = ExtensionRegistry::new();
        r.register(Arc::new(ContinueHandler {
            uri: "https://example/required",
            required: true,
        }));
        let activated = ActivatedExtensions::default();
        let err = r.enforce_required(&activated).unwrap_err();
        assert_eq!(err["status"], 400);
        assert_eq!(err["code"], "extension.required_not_activated");
    }

    #[test]
    fn enforce_required_when_present_returns_ok() {
        let mut r = ExtensionRegistry::new();
        r.register(Arc::new(ContinueHandler {
            uri: "https://example/required",
            required: true,
        }));
        let activated = ActivatedExtensions(vec!["https://example/required".to_string()]);
        assert!(r.enforce_required(&activated).is_ok());
    }

    #[test]
    fn echo_activated_intersects_with_registered() {
        let mut r = ExtensionRegistry::new();
        r.register(Arc::new(ContinueHandler {
            uri: "uri-a",
            required: false,
        }));
        r.register(Arc::new(ContinueHandler {
            uri: "uri-b",
            required: false,
        }));

        let activated = ActivatedExtensions(vec![
            "uri-a".to_string(),
            "uri-unknown".to_string(),
            "uri-b".to_string(),
        ]);
        let echoed = r.echo_activated(&activated);
        assert_eq!(echoed.0, vec!["uri-a", "uri-b"]);
    }
}

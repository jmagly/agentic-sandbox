//! v1 compatibility shim (#216 / W4.3).
//!
//! v1 endpoints continue to work unchanged. This module adds:
//!  - `Sunset: <date>` header on every v1 response (RFC 8594)
//!  - `Deprecated: true` header (Deprecation HTTP header draft, IETF)
//!  - Prometheus counter `aiwg_v1_path_requests_total{path}` per v1 hit
//!  - Documented v1→v2 path map in rustdoc + machine-readable form
//!    via [`path_map`].
//!
//! # Why a header-injection middleware (and not a proxy)
//!
//! v1 handlers already exist and serve the data we need. A proxy would
//! impose an extra HTTP hop and force payload translation in places
//! where the v1 and v2 semantics legitimately differ (e.g. v1
//! `/api/v1/sessions/{id}/dispatch` vs A2A `messages:send`). Instead,
//! the shim:
//!
//!  1. lets the v1 handler run as before
//!  2. tags every v1 response with `Sunset` + `Deprecated` headers so
//!     clients can react / log / migrate
//!  3. emits a Prometheus counter so operators can see who is still on
//!     v1 and prioritise migration work
//!
//! Actual semantic translation (e.g. WS mission events → SSE
//! SubscribeToTask, v1 HITL flow → A2A INPUT_REQUIRED) happens at the
//! client during migration; v1 clients keep getting v1 responses
//! until v3.0 removes the surface entirely.
//!
//! # Path translation map (v1 → v2)
//!
//! | v1 path                               | v2 destination                                              |
//! |---------------------------------------|-------------------------------------------------------------|
//! | `GET    /api/v1/agents`               | `GET    /api/v2/admin/instances`                            |
//! | `*      /api/v1/vms[...]`             | `*      /api/v2/admin/instances`                            |
//! | `GET    /api/v1/operations/{id}`      | `GET    /api/v2/admin/operations/{id}`                      |
//! | `*      /api/v1/storage/{scope}/...`  | `*      /api/v2/admin/storage/{scope}/...`                  |
//! | `GET    /api/v1/container-images`     | `GET    /api/v2/admin/container-images`                     |
//! | `POST   /api/v1/sessions/{id}/dispatch` | `POST /agents/{id}/v1/messages:send` (A2A; semantic shift) |
//! | `WS     /api/v1/ws/missions/{id}`     | `GET    /agents/{id}/v1/tasks/{tid}/subscribe` (SSE; transport shift) |
//! | `*      /api/v1/hitl/{id}`            | A2A `input-required` + `hitl-prompt/v1` extension           |
//! | `WS     ws://host:8121/sessions/{id}` | `WSS    /agents/{id}/sessions/{sid}/attach` (`pty-ws/v1`)   |
//!
//! Removal target: **v3.0** (≥12 months after v2.0 GA).

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Response},
    middleware::Next,
};
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

/// Default Sunset date — operator-configurable, defaults to 12 months
/// from v2.0 GA. Format: RFC 7231 IMF-fixdate (also referenced by
/// RFC 8594 for the `Sunset` header).
pub const DEFAULT_SUNSET: &str = "Sun, 09 May 2027 00:00:00 GMT";

/// The `Deprecated` HTTP header (IETF draft `draft-ietf-httpapi-deprecation-header`).
const DEPRECATED_HEADER: &str = "deprecated";

/// The `Sunset` HTTP header (RFC 8594).
const SUNSET_HEADER: &str = "sunset";

/// Atomic per-path counter for v1 hits. Exposed via [`V1Counter::snapshot`]
/// so the Prometheus exporter can render
/// `aiwg_v1_path_requests_total{path="..."}` lines without holding the
/// lock during text formatting.
#[derive(Default)]
pub struct V1Counter {
    by_path: RwLock<HashMap<String, u64>>,
}

impl V1Counter {
    /// Construct a fresh, empty counter wrapped in `Arc` for shared use.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Increment the count for `path`. Allocates a new map entry on
    /// the first hit per distinct path.
    pub fn inc(&self, path: &str) {
        let mut map = self.by_path.write();
        *map.entry(path.to_string()).or_insert(0) += 1;
    }

    /// Return a cloned snapshot of the current counts. The returned
    /// map is independent of the live counter — further `inc` calls
    /// do not mutate it.
    pub fn snapshot(&self) -> HashMap<String, u64> {
        self.by_path.read().clone()
    }
}

/// Configuration + state for the v1 compatibility middleware. Cheap to
/// clone (header value + `Arc`).
#[derive(Clone)]
pub struct CompatLayer {
    sunset_header: HeaderValue,
    counter: Arc<V1Counter>,
}

impl CompatLayer {
    /// Construct with [`DEFAULT_SUNSET`] and a fresh counter.
    pub fn new() -> Self {
        Self {
            sunset_header: HeaderValue::from_static(DEFAULT_SUNSET),
            counter: V1Counter::new(),
        }
    }

    /// Override the Sunset date. Invalid header values fall back to
    /// [`DEFAULT_SUNSET`] so a typo can't break the middleware.
    pub fn with_sunset(mut self, sunset: &str) -> Self {
        self.sunset_header = HeaderValue::from_str(sunset)
            .unwrap_or_else(|_| HeaderValue::from_static(DEFAULT_SUNSET));
        self
    }

    /// Use a shared external counter (e.g. one owned by the metrics
    /// module) instead of the per-layer default.
    pub fn with_counter(mut self, counter: Arc<V1Counter>) -> Self {
        self.counter = counter;
        self
    }

    /// Borrow the underlying counter — useful when the metrics
    /// exporter needs to read snapshots.
    pub fn counter(&self) -> Arc<V1Counter> {
        self.counter.clone()
    }
}

impl Default for CompatLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// Lookup table — v1 path → v2 equivalent. Public so operators can
/// query/print the canonical map (e.g. via a future `aiwg-doctor`
/// check or admin endpoint).
///
/// Entries use route templates with `{id}` / `{tid}` style placeholders
/// matching axum's path syntax.
pub fn path_map() -> &'static [(&'static str, &'static str)] {
    &[
        ("/api/v1/agents", "/api/v2/admin/instances"),
        ("/api/v1/vms", "/api/v2/admin/instances"),
        ("/api/v1/operations/{id}", "/api/v2/admin/operations/{id}"),
        (
            "/api/v1/storage/{scope}/{path}",
            "/api/v2/admin/storage/{scope}/{path}",
        ),
        (
            "/api/v1/container-images",
            "/api/v2/admin/container-images",
        ),
        (
            "/api/v1/sessions/{id}/dispatch",
            "/agents/{id}/v1/messages:send (A2A)",
        ),
        (
            "/api/v1/ws/missions/{id}",
            "/agents/{id}/v1/tasks/{tid}/subscribe (SSE)",
        ),
        (
            "/api/v1/hitl/{id}",
            "input-required + hitl-prompt/v1 extension",
        ),
        (
            "ws://host:8121/sessions/{id}",
            "wss://host/agents/{id}/sessions/{sid}/attach (pty-ws/v1)",
        ),
    ]
}

/// Axum middleware function: increments the v1 hit counter (when the
/// request targets `/api/v1/...`) and injects `Sunset` + `Deprecated`
/// response headers on the way back out.
///
/// Non-v1 paths (e.g. `/api/v2/admin/*`, `/healthz`, static UI assets)
/// pass through unchanged.
pub async fn compat_middleware(
    state: axum::extract::State<CompatLayer>,
    req: Request,
    next: Next,
) -> Response<Body> {
    let is_v1 = req.uri().path().starts_with("/api/v1/");
    let path_owned = if is_v1 {
        Some(req.uri().path().to_string())
    } else {
        None
    };

    if let Some(p) = &path_owned {
        state.counter.inc(p);
    }

    let mut response = next.run(req).await;

    if is_v1 {
        let headers = response.headers_mut();
        headers.insert(
            HeaderName::from_static(SUNSET_HEADER),
            state.sunset_header.clone(),
        );
        headers.insert(
            HeaderName::from_static(DEPRECATED_HEADER),
            HeaderValue::from_static("true"),
        );
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware::from_fn_with_state,
        routing::get,
        Router,
    };
    use chrono::NaiveDateTime;
    use tower::ServiceExt; // for `oneshot`

    /// Build a router with a representative v1 + v2 surface, wrapped
    /// in the compat layer. Used by the header/counter tests below.
    ///
    /// The middleware needs `CompatLayer` as its `State` — that's wired
    /// via `from_fn_with_state(layer, …)`. The router's own state is
    /// `()`, since the dummy handlers don't read application state.
    fn test_app(layer: CompatLayer) -> Router {
        async fn ok() -> &'static str {
            "ok"
        }

        Router::new()
            .route("/api/v1/agents", get(ok))
            .route("/api/v1/operations/{id}", get(ok))
            .route("/api/v2/admin/instances", get(ok))
            .route("/healthz", get(ok))
            .layer(from_fn_with_state(layer, compat_middleware))
    }

    /// Helper: issue a GET against the app and return the response.
    async fn get_path(app: &Router, path: &str) -> axum::http::Response<Body> {
        app.clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn middleware_adds_sunset_header() {
        let layer = CompatLayer::new();
        let app = test_app(layer);
        let resp = get_path(&app, "/api/v1/agents").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let sunset = resp
            .headers()
            .get(SUNSET_HEADER)
            .expect("Sunset header missing on v1 response");
        assert_eq!(sunset.to_str().unwrap(), DEFAULT_SUNSET);
    }

    #[tokio::test]
    async fn middleware_adds_deprecated_header() {
        let layer = CompatLayer::new();
        let app = test_app(layer);
        let resp = get_path(&app, "/api/v1/agents").await;
        let dep = resp
            .headers()
            .get(DEPRECATED_HEADER)
            .expect("Deprecated header missing on v1 response");
        assert_eq!(dep.to_str().unwrap(), "true");
    }

    #[tokio::test]
    async fn middleware_increments_counter() {
        let layer = CompatLayer::new();
        let counter = layer.counter();
        let app = test_app(layer);

        let _ = get_path(&app, "/api/v1/agents").await;
        let _ = get_path(&app, "/api/v1/agents").await;

        let snap = counter.snapshot();
        assert_eq!(
            snap.get("/api/v1/agents").copied().unwrap_or(0),
            2,
            "expected /api/v1/agents counter == 2, got snapshot: {:?}",
            snap
        );
    }

    #[tokio::test]
    async fn middleware_does_not_touch_v2() {
        let layer = CompatLayer::new();
        let counter = layer.counter();
        let app = test_app(layer);

        let resp = get_path(&app, "/api/v2/admin/instances").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get(SUNSET_HEADER).is_none(),
            "v2 response should not carry Sunset header"
        );
        assert!(
            resp.headers().get(DEPRECATED_HEADER).is_none(),
            "v2 response should not carry Deprecated header"
        );

        // Also covers /healthz and other non-v1 surfaces.
        let resp_health = get_path(&app, "/healthz").await;
        assert!(resp_health.headers().get(SUNSET_HEADER).is_none());

        let snap = counter.snapshot();
        assert!(
            !snap.contains_key("/api/v2/admin/instances"),
            "v2 path must not be recorded in v1 counter: {:?}",
            snap
        );
        assert!(
            !snap.contains_key("/healthz"),
            "non-v1 path must not be recorded in v1 counter: {:?}",
            snap
        );
    }

    #[test]
    fn path_map_is_well_formed() {
        for (v1, v2) in path_map() {
            let v1_ok = v1.starts_with("/api/v1/") || v1.starts_with("ws://");
            assert!(
                v1_ok,
                "v1 entry must start with /api/v1/ or ws://, got: {}",
                v1
            );

            let v2_ok = v2.starts_with("/api/v2/admin/")
                || v2.starts_with("/agents/")
                || v2.starts_with("wss://")
                || v2.contains("(A2A)")
                || v2.contains("(SSE)")
                || v2.contains("(pty-ws/v1)")
                || v2.contains("input-required");
            assert!(
                v2_ok,
                "v2 entry must point at /api/v2/admin/, /agents/, wss://, or describe an A2A/SSE/pty-ws/input-required target; got: {}",
                v2
            );
        }
    }

    #[test]
    fn counter_snapshot_returns_independent_map() {
        let counter = V1Counter::new();
        counter.inc("/api/v1/agents");
        let snap = counter.snapshot();
        assert_eq!(snap.get("/api/v1/agents").copied(), Some(1));

        // Mutate the counter after taking the snapshot — the snapshot
        // must NOT observe the new value.
        counter.inc("/api/v1/agents");
        counter.inc("/api/v1/agents");
        assert_eq!(
            snap.get("/api/v1/agents").copied(),
            Some(1),
            "snapshot must be independent of subsequent inc() calls"
        );

        // The live counter does reflect the new total.
        let snap2 = counter.snapshot();
        assert_eq!(snap2.get("/api/v1/agents").copied(), Some(3));
    }

    #[test]
    fn sunset_header_is_valid_rfc7231() {
        // RFC 7231 §7.1.1.1 IMF-fixdate, e.g. "Sun, 06 Nov 1994 08:49:37 GMT".
        // The trailing "GMT" is a literal in the grammar, not a parseable
        // timezone offset — so we parse with `NaiveDateTime` and treat the
        // "GMT" suffix as a fixed string. The value also has to be a real
        // calendar date / time, which `NaiveDateTime::parse_from_str` enforces.
        let parsed =
            NaiveDateTime::parse_from_str(DEFAULT_SUNSET, "%a, %d %b %Y %H:%M:%S GMT");
        assert!(
            parsed.is_ok(),
            "DEFAULT_SUNSET ({:?}) must parse as RFC 7231 IMF-fixdate: {:?}",
            DEFAULT_SUNSET,
            parsed.err()
        );
        // Header value must also be ASCII-safe (no embedded CR/LF, no
        // non-visible bytes) — HeaderValue::from_static would have panicked
        // at compile time if not, but exercise it explicitly so a future
        // operator overriding the date catches the mistake.
        assert!(
            DEFAULT_SUNSET.is_ascii() && !DEFAULT_SUNSET.contains(['\r', '\n']),
            "DEFAULT_SUNSET must be CR/LF-free ASCII for use as an HTTP header value"
        );
    }
}

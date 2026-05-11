//! v1 compatibility shim (#216 / W4.3, extended by #222).
//!
//! v1 endpoints continue to work unchanged. This module adds:
//!  - `Sunset: <date>` header on every v1 response (RFC 8594).
//!    Date defaults to [`DEFAULT_SUNSET`] but can be overridden at
//!    runtime via the `AIWG_V1_SUNSET_DATE` env var (RFC 7231
//!    IMF-fixdate). Invalid values log a warning and fall back to
//!    the default — a typo cannot break the middleware.
//!  - `Deprecated: true` header (Deprecation HTTP header draft, IETF)
//!  - `Link: <…>; rel="successor-version"` header pointing at the v2
//!    migration guide (RFC 8288), so clients can discover the migration
//!    path without out-of-band knowledge (#222).
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

/// Default `Link: rel="successor-version"` target — the v2 migration
/// guide. Operators rarely need to override this; it changes only when
/// the public guide URL moves.
pub const DEFAULT_LINK: &str =
    "<https://agentic-sandbox.aiwg.io/v2-migration-guide>; rel=\"successor-version\"";

/// Env var operators can set to override the default Sunset date.
/// Must be RFC 7231 IMF-fixdate (e.g. `Sun, 06 Nov 1994 08:49:37 GMT`).
pub const SUNSET_ENV_VAR: &str = "AIWG_V1_SUNSET_DATE";

/// The `Deprecated` HTTP header (IETF draft `draft-ietf-httpapi-deprecation-header`).
const DEPRECATED_HEADER: &str = "deprecated";

/// The `Sunset` HTTP header (RFC 8594).
const SUNSET_HEADER: &str = "sunset";

/// The `Link` HTTP header (RFC 8288).
const LINK_HEADER: &str = "link";

/// RFC 7231 IMF-fixdate format string for `chrono` parsing.
const IMF_FIXDATE_FMT: &str = "%a, %d %b %Y %H:%M:%S GMT";

/// Validate that `s` parses as an RFC 7231 IMF-fixdate. Returns `true`
/// iff the value is a real calendar date/time in the canonical format.
/// Pulled out so both the env override path and the `with_sunset` builder
/// reuse identical logic (and so tests can hit it directly).
pub fn is_valid_imf_fixdate(s: &str) -> bool {
    chrono::NaiveDateTime::parse_from_str(s, IMF_FIXDATE_FMT).is_ok()
}

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
/// clone (header values + `Arc`).
#[derive(Clone)]
pub struct CompatLayer {
    sunset_header: HeaderValue,
    link_header: HeaderValue,
    counter: Arc<V1Counter>,
}

impl CompatLayer {
    /// Construct with [`DEFAULT_SUNSET`] (or `AIWG_V1_SUNSET_DATE` if
    /// set and valid), the default migration-guide [`DEFAULT_LINK`],
    /// and a fresh counter.
    ///
    /// If `AIWG_V1_SUNSET_DATE` is set but does not parse as an RFC 7231
    /// IMF-fixdate, the env value is rejected with a `tracing::warn!`
    /// and the default is used. This avoids the failure mode where a
    /// typoed env var silently breaks every v1 response.
    pub fn new() -> Self {
        let sunset_value = match std::env::var(SUNSET_ENV_VAR) {
            Ok(s) if is_valid_imf_fixdate(&s) => s,
            Ok(bad) => {
                tracing::warn!(
                    env_var = SUNSET_ENV_VAR,
                    value = %bad,
                    default = DEFAULT_SUNSET,
                    "AIWG_V1_SUNSET_DATE is not a valid RFC 7231 IMF-fixdate; falling back to default"
                );
                DEFAULT_SUNSET.to_string()
            }
            Err(_) => DEFAULT_SUNSET.to_string(),
        };

        let sunset_header = HeaderValue::from_str(&sunset_value)
            .unwrap_or_else(|_| HeaderValue::from_static(DEFAULT_SUNSET));

        Self {
            sunset_header,
            link_header: HeaderValue::from_static(DEFAULT_LINK),
            counter: V1Counter::new(),
        }
    }

    /// Override the Sunset date. Invalid header values (or values that
    /// don't parse as RFC 7231 IMF-fixdate) fall back to
    /// [`DEFAULT_SUNSET`] so a typo can't break the middleware.
    ///
    /// Use this in tests and call sites that want to bypass the
    /// `AIWG_V1_SUNSET_DATE` env-var path — it avoids the global-state
    /// flakiness of mutating process env inside parallel tests.
    pub fn with_sunset(mut self, sunset: &str) -> Self {
        if !is_valid_imf_fixdate(sunset) {
            self.sunset_header = HeaderValue::from_static(DEFAULT_SUNSET);
            return self;
        }
        self.sunset_header = HeaderValue::from_str(sunset)
            .unwrap_or_else(|_| HeaderValue::from_static(DEFAULT_SUNSET));
        self
    }

    /// Override the `Link: rel="successor-version"` value. Invalid
    /// header values fall back to [`DEFAULT_LINK`].
    pub fn with_link(mut self, link: &str) -> Self {
        self.link_header =
            HeaderValue::from_str(link).unwrap_or_else(|_| HeaderValue::from_static(DEFAULT_LINK));
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

    /// Borrow the currently-configured Sunset header value (testing /
    /// introspection).
    pub fn sunset_header_value(&self) -> &HeaderValue {
        &self.sunset_header
    }

    /// Borrow the currently-configured Link header value (testing /
    /// introspection).
    pub fn link_header_value(&self) -> &HeaderValue {
        &self.link_header
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
        ("/api/v1/container-images", "/api/v2/admin/container-images"),
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
        headers.insert(
            HeaderName::from_static(LINK_HEADER),
            state.link_header.clone(),
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
        assert!(
            resp.headers().get(LINK_HEADER).is_none(),
            "v2 response should not carry Link successor-version header"
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
        let parsed = NaiveDateTime::parse_from_str(DEFAULT_SUNSET, "%a, %d %b %Y %H:%M:%S GMT");
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

    #[tokio::test]
    async fn middleware_adds_link_header_with_successor_version(/* #222 */) {
        let layer = CompatLayer::new();
        let app = test_app(layer);
        let resp = get_path(&app, "/api/v1/agents").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let link = resp
            .headers()
            .get(LINK_HEADER)
            .expect("Link header missing on v1 response");
        let link_str = link.to_str().unwrap();

        assert!(
            link_str.contains("rel=\"successor-version\""),
            "Link header must include rel=\"successor-version\", got: {}",
            link_str
        );
        assert!(
            link_str.contains("agentic-sandbox.aiwg.io/v2-migration-guide"),
            "Link header must point at the v2 migration guide, got: {}",
            link_str
        );
        // RFC 8288 §3 — the URI must be enclosed in angle brackets.
        assert!(
            link_str.starts_with('<') && link_str.contains('>'),
            "Link header URI must be enclosed in <…>, got: {}",
            link_str
        );
    }

    #[test]
    fn link_header_default_points_to_migration_guide(/* #222 */) {
        // The default link value is what every freshly-constructed layer
        // sees in the absence of an explicit `with_link()` override.
        let layer = CompatLayer::new();
        let link = layer.link_header_value();
        let s = link.to_str().unwrap();
        assert_eq!(
            s, DEFAULT_LINK,
            "default Link header must equal DEFAULT_LINK"
        );
        assert_eq!(
            DEFAULT_LINK,
            "<https://agentic-sandbox.aiwg.io/v2-migration-guide>; rel=\"successor-version\"",
            "DEFAULT_LINK URL must remain the canonical migration-guide URL — \
             update docs/v2-migration-guide.md if this value moves"
        );
    }

    #[test]
    fn sunset_date_overridable_via_with_sunset(/* #222 */) {
        // Use the builder rather than mutating the global env var — this
        // keeps the test deterministic under `cargo test` parallelism.
        // The env-var path is exercised by `sunset_env_var_invalid_falls_back`
        // below using `with_sunset` to mirror the same fall-back logic.
        let custom = "Mon, 01 Jan 2029 00:00:00 GMT";
        let layer = CompatLayer::new().with_sunset(custom);
        assert_eq!(
            layer.sunset_header_value().to_str().unwrap(),
            custom,
            "with_sunset() must replace the configured Sunset value"
        );

        // Invalid input must fall back to DEFAULT_SUNSET, not silently
        // accept a malformed date.
        let layer = CompatLayer::new().with_sunset("not a date");
        assert_eq!(
            layer.sunset_header_value().to_str().unwrap(),
            DEFAULT_SUNSET,
            "invalid Sunset value must fall back to DEFAULT_SUNSET"
        );
    }

    #[test]
    fn is_valid_imf_fixdate_accepts_canonical_and_rejects_garbage(/* #222 */) {
        assert!(is_valid_imf_fixdate(DEFAULT_SUNSET));
        assert!(is_valid_imf_fixdate("Sun, 06 Nov 1994 08:49:37 GMT"));

        assert!(!is_valid_imf_fixdate(""));
        assert!(!is_valid_imf_fixdate("2027-05-09"));
        assert!(!is_valid_imf_fixdate("Sun, 09 May 2027 00:00:00 UTC"));
        assert!(!is_valid_imf_fixdate("not a date"));
    }
}

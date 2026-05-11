//! Per-instance context and routing registry.
//!
//! Filled in by W3.5 (#212): routing layer + InstanceRegistry.
//! ADR-022 Surface 2 routing layer.
//!
//! Each running agent instance is represented by an [`InstanceContext`] holding
//! its identity and runtime metadata. The [`InstanceRegistry`] maps instance
//! IDs to contexts so the HTTP layer can route inbound A2A requests to the
//! correct instance via the [`InstanceLayer`] tower middleware.
//!
//! Co-existence with #209: this module owns the routing-layer fields and the
//! primary `impl InstanceContext` block. #209 appends additional fields
//! (`cached_card`, `signing_key`) and a separate `impl` block at the bottom of
//! the file.

use std::collections::HashMap;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::extract::FromRequestParts;
use axum::http::{request::Parts, Request, Response, StatusCode};
use axum::response::IntoResponse;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tower::{Layer, Service};

/// Stable identifier for an executor instance (one running agent).
pub type InstanceId = String;

/// Runtime kind for an instance — VM-backed or container-backed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    /// Full VM (QEMU/KVM via libvirt).
    Vm,
    /// Container-backed instance.
    Container,
}

/// Per-instance context.
///
/// Holds the identity and runtime metadata used to route requests and build
/// AgentCards for a single running agent instance. Wrapped in `Arc` and stored
/// in [`InstanceRegistry`].
///
/// Field ownership:
/// - #212 owns the routing-layer fields below.
/// - #209 will append `cached_card` and `signing_key` to this struct.
pub struct InstanceContext {
    /// Stable instance ID (usually matches the sandbox/agent ID upstream).
    pub instance_id: String,
    /// Runtime backing this instance.
    pub runtime_kind: RuntimeKind,
    /// Loadout name (e.g. `claude-only`, `dual-review`).
    pub loadout: String,
    /// Optional image reference (container image, VM image ref).
    pub image_ref: Option<String>,
    /// Host address or hostname serving this instance.
    pub host: String,
    /// Creation timestamp.
    pub created_at: chrono::DateTime<chrono::Utc>,

    // #209 additions
    /// Cached signed AgentCard. Cleared via [`Self::invalidate_card`] on
    /// any capability/skill change so the next [`Self::signed_card`] call
    /// rebuilds and re-signs.
    pub cached_card: parking_lot::RwLock<Option<crate::agent_card::SignedAgentCard>>,
    /// Per-instance Ed25519 key used to sign the AgentCard. Held in an
    /// [`Arc`] so multiple async tasks can share without cloning the
    /// underlying key material.
    pub signing_key: Arc<crate::agent_card::SigningKey>,
}

impl InstanceContext {
    /// Construct a routing-layer context, loading or generating a persistent
    /// Ed25519 signing key under `<signing_keys_dir>/<instance_id>/` (#253).
    ///
    /// On every restart the binary calls this with the same
    /// `signing_keys_dir` (typically `<secrets_dir>/instances`) so each
    /// instance reuses its key across restarts — preventing signature
    /// rotation churn for clients that have cached the AgentCard JWK.
    pub fn new(
        instance_id: impl Into<String>,
        runtime_kind: RuntimeKind,
        loadout: impl Into<String>,
        image_ref: Option<String>,
        host: impl Into<String>,
        signing_keys_dir: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let instance_id: String = instance_id.into();
        let key_dir = signing_keys_dir.join(&instance_id);
        let signing_key =
            crate::agent_card::SigningKey::load_or_generate(&key_dir, instance_id.clone())?;
        Ok(Self {
            instance_id,
            runtime_kind,
            loadout: loadout.into(),
            image_ref,
            host: host.into(),
            created_at: chrono::Utc::now(),
            cached_card: parking_lot::RwLock::new(None),
            signing_key: Arc::new(signing_key),
        })
    }

    /// Construct an ephemeral context with an in-memory signing key (#253).
    ///
    /// Used by tests and harness builds where on-disk key persistence is
    /// unwanted. Production code must use [`Self::new`] with a persistent
    /// `signing_keys_dir`.
    pub fn new_ephemeral(
        instance_id: impl Into<String>,
        runtime_kind: RuntimeKind,
        loadout: impl Into<String>,
        image_ref: Option<String>,
        host: impl Into<String>,
    ) -> Self {
        let instance_id: String = instance_id.into();
        let signing_key = crate::agent_card::SigningKey::generate_ed25519(instance_id.clone())
            .expect("ed25519 key generation must succeed");
        Self {
            instance_id,
            runtime_kind,
            loadout: loadout.into(),
            image_ref,
            host: host.into(),
            created_at: chrono::Utc::now(),
            cached_card: parking_lot::RwLock::new(None),
            signing_key: Arc::new(signing_key),
        }
    }
}

// #209 additions
impl InstanceContext {
    /// Drop the cached AgentCard so the next [`Self::signed_card`] call
    /// rebuilds and re-signs from fresh inputs.
    pub fn invalidate_card(&self) {
        *self.cached_card.write() = None;
    }

    /// Return a signed AgentCard, building and signing on cache miss.
    ///
    /// On cache hit, returns a clone of the cached value. Callers that
    /// need to rebuild (e.g., capability changes) should call
    /// [`Self::invalidate_card`] first.
    pub fn signed_card(
        &self,
        inputs: &crate::agent_card::AgentCardInputs,
    ) -> anyhow::Result<crate::agent_card::SignedAgentCard> {
        if let Some(cached) = self.cached_card.read().as_ref() {
            return Ok(cached.clone());
        }
        let card = crate::agent_card::build_agent_card(inputs);
        let signed = crate::agent_card::sign_agent_card(card, &self.signing_key)?;
        *self.cached_card.write() = Some(signed.clone());
        Ok(signed)
    }
}

/// Registry mapping instance IDs to contexts.
///
/// Cheaply cloneable (internally `Arc<RwLock<HashMap<...>>>`). Concurrent
/// reads scale via `parking_lot::RwLock`.
#[derive(Default, Clone)]
pub struct InstanceRegistry {
    inner: Arc<RwLock<HashMap<String, Arc<InstanceContext>>>>,
}

impl InstanceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Insert (or replace) the context for an instance ID.
    pub fn insert(&self, ctx: Arc<InstanceContext>) {
        let id = ctx.instance_id.clone();
        self.inner.write().insert(id, ctx);
    }

    /// Look up an instance by ID.
    pub fn get(&self, instance_id: &str) -> Option<Arc<InstanceContext>> {
        self.inner.read().get(instance_id).cloned()
    }

    /// Remove an instance from the registry.
    pub fn remove(&self, instance_id: &str) -> Option<Arc<InstanceContext>> {
        self.inner.write().remove(instance_id)
    }

    /// Number of registered instances.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Snapshot of all registered instance IDs.
    pub fn list_ids(&self) -> Vec<String> {
        self.inner.read().keys().cloned().collect()
    }
}

/// Tower [`Layer`] that injects `Arc<InstanceContext>` into request extensions
/// for any path matching `/agents/{id}/...`. Unknown instance IDs receive a
/// 404 problem+json envelope. Non-`/agents/` paths pass through unmodified.
#[derive(Clone)]
pub struct InstanceLayer {
    registry: InstanceRegistry,
}

impl InstanceLayer {
    /// Build a layer backed by the given registry.
    pub fn new(registry: InstanceRegistry) -> Self {
        Self { registry }
    }
}

impl<S> Layer<S> for InstanceLayer {
    type Service = InstanceMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        InstanceMiddleware {
            inner,
            registry: self.registry.clone(),
        }
    }
}

/// Tower [`Service`] produced by [`InstanceLayer`]. See the layer docs.
#[derive(Clone)]
pub struct InstanceMiddleware<S> {
    inner: S,
    registry: InstanceRegistry,
}

/// Extract the `{id}` segment from a path of the form `/agents/{id}/...` or
/// `/agents/{id}`. Returns `None` if the path does not match.
fn extract_instance_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/agents/")?;
    if rest.is_empty() {
        return None;
    }
    let id = match rest.find('/') {
        Some(idx) => &rest[..idx],
        None => rest,
    };
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

impl<S, B> Service<Request<B>> for InstanceMiddleware<S>
where
    S: Service<Request<B>, Response = Response<axum::body::Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        // Take out the inner service to satisfy the move-in-future requirement.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        let registry = self.registry.clone();

        Box::pin(async move {
            let path = req.uri().path().to_string();
            match extract_instance_id(&path) {
                Some(id) => match registry.get(id) {
                    Some(ctx) => {
                        req.extensions_mut().insert(ctx);
                        inner.call(req).await
                    }
                    None => {
                        let body = serde_json::json!({
                            "type": "instance.not_found",
                            "title": "Instance not found",
                            "status": 404,
                            "detail": format!("Instance '{}' not registered", sanitize(id)),
                            "instance_id": id,
                        });
                        let resp = (
                            StatusCode::NOT_FOUND,
                            [(
                                axum::http::header::CONTENT_TYPE,
                                "application/problem+json",
                            )],
                            body.to_string(),
                        )
                            .into_response();
                        Ok(resp)
                    }
                },
                None => inner.call(req).await,
            }
        })
    }
}

/// Sanitize an instance ID for inclusion in the 404 detail string. Keeps
/// alphanumerics, dashes, underscores, dots; replaces the rest with `_`.
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Axum extractor that pulls the [`InstanceContext`] inserted by
/// [`InstanceLayer`] out of request extensions. Returns 500 if the layer was
/// not installed (i.e. the extension is missing).
pub struct InstanceExt(pub Arc<InstanceContext>);

impl<S> FromRequestParts<S> for InstanceExt
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Arc<InstanceContext>>()
            .cloned()
            .map(InstanceExt)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use std::thread;
    use tower::ServiceExt;

    fn mk_ctx(id: &str) -> Arc<InstanceContext> {
        Arc::new(InstanceContext::new_ephemeral(
            id,
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "127.0.0.1",
        ))
    }

    #[test]
    fn registry_insert_get_remove() {
        let reg = InstanceRegistry::new();
        assert_eq!(reg.len(), 0);

        let ctx = mk_ctx("inst-1");
        reg.insert(ctx.clone());
        assert_eq!(reg.len(), 1);

        let got = reg.get("inst-1").expect("present after insert");
        assert_eq!(got.instance_id, "inst-1");
        assert_eq!(got.runtime_kind, RuntimeKind::Vm);

        let removed = reg.remove("inst-1").expect("removed returns prior");
        assert_eq!(removed.instance_id, "inst-1");
        assert_eq!(reg.len(), 0);
        assert!(reg.get("inst-1").is_none());
    }

    #[test]
    fn registry_concurrent_reads() {
        let reg = InstanceRegistry::new();
        for n in 0..4 {
            reg.insert(mk_ctx(&format!("inst-{n}")));
        }

        let mut handles = Vec::new();
        for _ in 0..8 {
            let r = reg.clone();
            handles.push(thread::spawn(move || {
                for n in 0..4 {
                    let id = format!("inst-{n}");
                    assert!(r.get(&id).is_some(), "all reads succeed");
                }
            }));
        }
        for h in handles {
            h.join().expect("thread joined");
        }
        assert_eq!(reg.len(), 4);
    }

    fn build_app(reg: InstanceRegistry) -> Router {
        Router::new()
            .route(
                "/agents/{id}/ping",
                get(|InstanceExt(ctx): InstanceExt| async move {
                    format!("ok:{}", ctx.instance_id)
                }),
            )
            .route("/health", get(|| async { "healthy" }))
            .layer(InstanceLayer::new(reg))
    }

    #[tokio::test]
    async fn middleware_known_instance_passes_through() {
        let reg = InstanceRegistry::new();
        reg.insert(mk_ctx("inst-known"));

        let app = build_app(reg);
        let req = Request::builder()
            .uri("/agents/inst-known/ping")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok:inst-known");
    }

    #[tokio::test]
    async fn middleware_unknown_instance_returns_404() {
        let reg = InstanceRegistry::new();

        let app = build_app(reg);
        let req = Request::builder()
            .uri("/agents/missing-1/ping")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(ct, "application/problem+json");
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["type"], "instance.not_found");
        assert_eq!(v["status"], 404);
        assert_eq!(v["instance_id"], "missing-1");
    }

    #[tokio::test]
    async fn middleware_non_agents_path_passes_through() {
        let reg = InstanceRegistry::new();

        let app = build_app(reg);
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"healthy");
    }

    #[tokio::test]
    async fn extractor_returns_context() {
        // Covered as part of middleware_known_instance_passes_through, but
        // we add an explicit assertion on a second instance here to make the
        // contract obvious.
        let reg = InstanceRegistry::new();
        reg.insert(mk_ctx("inst-A"));
        reg.insert(mk_ctx("inst-B"));

        let app = build_app(reg);
        let req = Request::builder()
            .uri("/agents/inst-B/ping")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok:inst-B");
    }

    #[tokio::test]
    async fn extractor_500_when_middleware_missing() {
        // Build a router WITHOUT the InstanceLayer — the extractor should
        // return INTERNAL_SERVER_ERROR.
        let app: Router = Router::new().route(
            "/agents/{id}/ping",
            get(|InstanceExt(ctx): InstanceExt| async move {
                format!("ok:{}", ctx.instance_id)
            }),
        );
        let req = Request::builder()
            .uri("/agents/anything/ping")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn instance_context_new_persists_key_on_first_call() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = InstanceContext::new(
            "inst-persist-1",
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "127.0.0.1",
            tmp.path(),
        )
        .expect("construct + persist");

        let key_dir = tmp.path().join("inst-persist-1");
        assert!(key_dir.join("signing.pem").exists());
        assert!(key_dir.join("signing.jwk.json").exists());
        assert_eq!(ctx.signing_key.kid(), "inst-persist-1");
    }

    #[test]
    fn instance_context_new_reuses_key_on_second_call() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx1 = InstanceContext::new(
            "inst-reuse-2",
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "127.0.0.1",
            tmp.path(),
        )
        .expect("first construct");
        let pub1 = ctx1.signing_key.public_jwk().unwrap();

        let ctx2 = InstanceContext::new(
            "inst-reuse-2",
            RuntimeKind::Vm,
            "agentic-dev",
            None,
            "127.0.0.1",
            tmp.path(),
        )
        .expect("second construct");
        let pub2 = ctx2.signing_key.public_jwk().unwrap();

        assert_eq!(ctx1.signing_key.kid(), ctx2.signing_key.kid());
        assert_eq!(pub1["x"], pub2["x"], "public key bytes must be identical");
    }

    #[test]
    fn extract_instance_id_variants() {
        assert_eq!(extract_instance_id("/agents/abc/ping"), Some("abc"));
        assert_eq!(extract_instance_id("/agents/abc"), Some("abc"));
        assert_eq!(extract_instance_id("/agents/abc/"), Some("abc"));
        assert_eq!(extract_instance_id("/agents/"), None);
        assert_eq!(extract_instance_id("/agents"), None);
        assert_eq!(extract_instance_id("/health"), None);
        assert_eq!(extract_instance_id("/"), None);
    }
}

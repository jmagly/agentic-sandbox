//! JWKS endpoints — RFC 7517 (#253).
//!
//! Aggregates per-instance Ed25519 public keys so JWS-verifying clients
//! (AIWG, conformance harness, external A2A peers) can fetch a single
//! JWKS to validate AgentCard signatures.
//!
//! Routes:
//! - `GET /.well-known/jwks.json`             — server-wide JWKS (all
//!   currently-registered instances).
//! - `GET /agents/{instance_id}/.well-known/jwks.json` — just this
//!   instance's key.

use axum::body::Body;
use axum::extract::State;
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::bindings::rest::{error_response, AppState};
use crate::instance::InstanceExt;

/// `GET /.well-known/jwks.json` — server-wide JWKS.
pub async fn all_instances(State(state): State<AppState>) -> Response {
    let ids = state.instance_registry.list_ids();
    let mut keys: Vec<Value> = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(ctx) = state.instance_registry.get(&id) {
            match ctx.signing_key.public_jwk() {
                Ok(jwk) => keys.push(jwk),
                Err(e) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "https://agentic-sandbox.aiwg.io/errors/internal",
                        "Internal server error",
                        format!("Failed to serialize public JWK for {id}: {e}"),
                        "internal.error",
                        None,
                        Some(&id),
                    );
                }
            }
        }
    }
    let body = json!({ "keys": keys });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(body.to_string()))
        .unwrap()
        .into_response()
}

/// `GET /agents/{instance_id}/.well-known/jwks.json` — per-instance JWKS.
pub async fn single_instance(InstanceExt(ctx): InstanceExt) -> Response {
    match ctx.signing_key.public_jwk() {
        Ok(jwk) => {
            let body = json!({ "keys": [jwk] });
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(Body::from(body.to_string()))
                .unwrap()
                .into_response()
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "https://agentic-sandbox.aiwg.io/errors/internal",
            "Internal server error",
            format!("Failed to serialize public JWK: {e}"),
            "internal.error",
            None,
            Some(&ctx.instance_id),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::rest::router_with_bridge;
    use crate::bindings::pty_bridge::NoOpPtyBridge;
    use crate::instance::{InstanceContext, InstanceRegistry, RuntimeKind};
    use crate::store::idempotency::IdempotencyCache;
    use crate::store::task_store::TaskStore;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn mk_router(instance_ids: &[&str]) -> axum::Router {
        let reg = InstanceRegistry::new();
        for id in instance_ids {
            reg.insert(Arc::new(InstanceContext::new_ephemeral(
                *id,
                RuntimeKind::Vm,
                "agentic-dev",
                None,
                format!("{}.example.test", id),
            )));
        }
        let store = Arc::new(TaskStore::open_in_memory().unwrap());
        let idem = Arc::new(IdempotencyCache::new(store.clone()));
        router_with_bridge(reg, store, idem, Arc::new(NoOpPtyBridge))
    }

    async fn read_body_json(resp: Response) -> Value {
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn jwks_aggregate_returns_all_instances() {
        let app = mk_router(&["inst-a", "inst-b", "inst-c"]);
        let req = Request::builder()
            .method("GET")
            .uri("/.well-known/jwks.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body_json(resp).await;
        let keys = v["keys"].as_array().expect("keys array");
        assert_eq!(keys.len(), 3, "expected 3 keys, got {}", keys.len());
        let mut kids: Vec<&str> =
            keys.iter().map(|k| k["kid"].as_str().unwrap()).collect();
        kids.sort();
        assert_eq!(kids, vec!["inst-a", "inst-b", "inst-c"]);
        for k in keys {
            assert_eq!(k["alg"], "EdDSA");
            assert_eq!(k["kty"], "OKP");
            assert!(k.get("d").is_none(), "must not leak private `d`");
        }
    }

    #[tokio::test]
    async fn jwks_single_instance_returns_one_key() {
        let app = mk_router(&["inst-single"]);
        let req = Request::builder()
            .method("GET")
            .uri("/agents/inst-single/.well-known/jwks.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body_json(resp).await;
        let keys = v["keys"].as_array().expect("keys array");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["kid"], "inst-single");
    }

    #[tokio::test]
    async fn jwks_empty_registry_returns_empty_keys_array() {
        let app = mk_router(&[]);
        let req = Request::builder()
            .method("GET")
            .uri("/.well-known/jwks.json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = read_body_json(resp).await;
        let keys = v["keys"].as_array().expect("keys array");
        assert!(keys.is_empty(), "expected empty keys array");
    }
}

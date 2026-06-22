//! HTTP API for startup profile policies.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;

use super::server::AppState;
use crate::startup_profiles::{StartupProfile, StartupProfileError, UpsertStartupProfileRequest};

#[derive(Debug, Serialize)]
struct StartupProfileListResponse {
    startup_profiles: Vec<StartupProfile>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_startup_profiles).post(create_startup_profile))
        .route(
            "/{id}",
            get(get_startup_profile)
                .put(update_startup_profile)
                .delete(delete_startup_profile),
        )
}

async fn list_startup_profiles(State(state): State<AppState>) -> Response {
    Json(StartupProfileListResponse {
        startup_profiles: state.startup_profiles.list(),
    })
    .into_response()
}

async fn create_startup_profile(
    State(state): State<AppState>,
    Json(request): Json<UpsertStartupProfileRequest>,
) -> Response {
    match state.startup_profiles.create(request) {
        Ok(profile) => (StatusCode::CREATED, Json(profile)).into_response(),
        Err(err) => startup_profile_error(err),
    }
}

async fn get_startup_profile(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.startup_profiles.get(&id) {
        Ok(profile) => Json(profile).into_response(),
        Err(err) => startup_profile_error(err),
    }
}

async fn update_startup_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpsertStartupProfileRequest>,
) -> Response {
    match state.startup_profiles.update(&id, request) {
        Ok(profile) => Json(profile).into_response(),
        Err(err) => startup_profile_error(err),
    }
}

async fn delete_startup_profile(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.startup_profiles.delete(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => startup_profile_error(err),
    }
}

fn startup_profile_error(err: StartupProfileError) -> Response {
    let status = match err {
        StartupProfileError::NotFound(_) => StatusCode::NOT_FOUND,
        StartupProfileError::AlreadyExists(_) => StatusCode::CONFLICT,
        StartupProfileError::Validation(_) => StatusCode::BAD_REQUEST,
        StartupProfileError::Persistence(_) | StartupProfileError::Serialization(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        Json(ErrorResponse {
            error: err.to_string(),
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let registry = Arc::new(crate::registry::AgentRegistry::new());
        AppState {
            registry: registry.clone(),
            output_agg: Arc::new(crate::output::OutputAggregator::new(64)),
            dispatcher: Arc::new(crate::dispatch::CommandDispatcher::new(registry)),
            orchestrator: None,
            metrics: None,
            operation_store: Some(Arc::new(super::super::operations::OperationStore::new())),
            audit_logger: None,
            credential_broker: Arc::new(crate::credentials::CredentialBroker::new_in_memory()),
            startup_profiles: Arc::new(
                crate::startup_profiles::StartupProfileStore::new_in_memory(),
            ),
            ssh_gateway_leases: Arc::new(crate::ssh_gateway::SshGatewayLeaseStore::new_in_memory()),
            bootstrap_token_store: None,
            grpc_ca_backend: None,
            screen_registry: None,
            hitl_store: None,
            aiwg_handle: None,
            mission_store: None,
            session_registry: None,
            agentshare_root: None,
            tasks_root: None,
            operator_auth: None,
            mtls_config: super::super::operator_auth::MtlsConfig::default(),
            unix_peer_creds_config: super::super::operator_auth::UnixPeerCredsConfig::default(),
            executor_instance_registry: None,
            executor_signing_keys_dir: None,
            executor_idempotency: None,
            host_runtime_supervisor: None,
            v1_counter: None,
        }
    }

    fn profile_payload() -> Value {
        json!({
            "id": "startup_codex",
            "trigger": "on_instance_ready",
            "target": {
                "loadout": "automation-control",
                "provider": "codex"
            },
            "session": {
                "command": "agentic-codex-automation --profile startup_codex",
                "workdir": "/home/agent/workspace"
            },
            "credential_refs": [
                {
                    "id": "cred_openai_api",
                    "provider": "codex",
                    "allowed_use": "provider_api",
                    "target": { "type": "env", "name": "OPENAI_API_KEY" }
                }
            ],
            "readiness_probes": [
                {
                    "kind": "command",
                    "command": "agentic-provider-readiness codex",
                    "timeout_seconds": 10
                }
            ]
        })
    }

    #[tokio::test]
    async fn startup_profile_api_crud_is_metadata_only() {
        let app = Router::new()
            .nest("/api/v2/startup-profiles", router())
            .with_state(test_state());
        let body = serde_json::to_vec(&profile_payload()).unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/startup-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("sk-"));
        assert!(!text.contains("plaintext"));
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["id"], "startup_codex");
        assert_eq!(json["status"]["state"], "pending");
        assert_eq!(
            json["credential_refs"][0]["target"]["name"],
            "OPENAI_API_KEY"
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v2/startup-profiles/startup_codex")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/v2/startup-profiles/startup_codex")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn startup_profile_api_rejects_inline_secret_fields() {
        let app = Router::new()
            .nest("/api/v2/startup-profiles", router())
            .with_state(test_state());
        let mut payload = profile_payload();
        payload["credential_refs"][0]["value"] = json!("sk-not-real");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/startup-profiles")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

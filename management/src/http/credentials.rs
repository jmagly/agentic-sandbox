//! HTTP API for workload credential metadata.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Serialize;

use super::server::AppState;
use crate::credentials::{
    CredentialError, CredentialLeaseResponse, CredentialMetadataResponse,
    IssueCredentialLeaseRequest, UpsertCredentialRequest,
};

#[derive(Debug, Serialize)]
struct CredentialListResponse {
    credentials: Vec<CredentialMetadataResponse>,
}

#[derive(Debug, Serialize)]
struct CredentialLeaseListResponse {
    leases: Vec<CredentialLeaseResponse>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_credentials).post(create_credential))
        .route("/leases", get(list_leases))
        .route("/leases/{lease_id}", get(get_lease).delete(revoke_lease))
        .route(
            "/{id}/leases",
            get(list_leases_for_credential).post(issue_lease),
        )
        .route(
            "/{id}",
            get(get_credential)
                .put(update_credential)
                .delete(delete_credential),
        )
}

async fn list_credentials(State(state): State<AppState>) -> Response {
    Json(CredentialListResponse {
        credentials: state.credential_broker.list(),
    })
    .into_response()
}

async fn create_credential(
    State(state): State<AppState>,
    Json(request): Json<UpsertCredentialRequest>,
) -> Response {
    match state.credential_broker.create(request) {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(err) => credential_error(err),
    }
}

async fn list_leases(State(state): State<AppState>) -> Response {
    Json(CredentialLeaseListResponse {
        leases: state.credential_broker.list_leases(),
    })
    .into_response()
}

async fn get_lease(State(state): State<AppState>, Path(lease_id): Path<String>) -> Response {
    match state.credential_broker.get_lease(&lease_id) {
        Ok(response) => Json(response).into_response(),
        Err(err) => credential_error(err),
    }
}

async fn revoke_lease(State(state): State<AppState>, Path(lease_id): Path<String>) -> Response {
    match state.credential_broker.revoke_lease(&lease_id) {
        Ok(response) => Json(response).into_response(),
        Err(err) => credential_error(err),
    }
}

async fn list_leases_for_credential(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let leases = state
        .credential_broker
        .list_leases()
        .into_iter()
        .filter(|lease| lease.credential_id == id)
        .collect();
    Json(CredentialLeaseListResponse { leases }).into_response()
}

async fn issue_lease(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<IssueCredentialLeaseRequest>,
) -> Response {
    match state.credential_broker.issue_lease(&id, request) {
        Ok(response) => (StatusCode::CREATED, Json(response)).into_response(),
        Err(err) => credential_error(err),
    }
}

async fn get_credential(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.credential_broker.get(&id) {
        Ok(response) => Json(response).into_response(),
        Err(err) => credential_error(err),
    }
}

async fn update_credential(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpsertCredentialRequest>,
) -> Response {
    match state.credential_broker.update(&id, request) {
        Ok(response) => Json(response).into_response(),
        Err(err) => credential_error(err),
    }
}

async fn delete_credential(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.credential_broker.delete(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => credential_error(err),
    }
}

fn credential_error(err: CredentialError) -> Response {
    let status = match err {
        CredentialError::MissingId
        | CredentialError::MissingProvider
        | CredentialError::MissingType => StatusCode::BAD_REQUEST,
        CredentialError::AlreadyExists(_) => StatusCode::CONFLICT,
        CredentialError::NotFound(_) | CredentialError::LeaseNotFound(_) => StatusCode::NOT_FOUND,
        CredentialError::NotConfigured(_)
        | CredentialError::LeaseDenied(_)
        | CredentialError::UnsupportedBackend(_) => StatusCode::FORBIDDEN,
        CredentialError::Persistence(_) | CredentialError::Serialization(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        CredentialError::BackendRead { .. } => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(ErrorResponse {
            error: err.to_string(),
        }),
    )
        .into_response()
}

use axum::response::IntoResponse;

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

    #[tokio::test]
    async fn credential_api_never_returns_write_only_value() {
        let app = Router::new()
            .nest("/api/v2/credentials", router())
            .with_state(test_state());
        let body = serde_json::to_vec(&json!({
            "id": "cred_openai_api",
            "provider": "openai",
            "type": "api_key",
            "scopes": ["codex:run"],
            "allowed_uses": ["session.launch"],
            "value": {
                "kind": "write_only",
                "plaintext": "sk-http-secret"
            }
        }))
        .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/credentials")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("sk-http-secret"));
        assert!(!text.contains("plaintext"));
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["id"], "cred_openai_api");
        assert_eq!(json["configured"], true);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v2/credentials")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("sk-http-secret"));
        assert!(!text.contains("plaintext"));
    }

    #[tokio::test]
    async fn credential_api_issues_and_revokes_metadata_only_lease() {
        let app = Router::new()
            .nest("/api/v2/credentials", router())
            .with_state(test_state());
        let credential_body = serde_json::to_vec(&json!({
            "id": "cred_openai_api",
            "provider": "openai",
            "type": "api_key",
            "allowed_uses": ["session.launch"],
            "value": {
                "kind": "write_only",
                "plaintext": "sk-http-lease-secret"
            }
        }))
        .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/credentials")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(credential_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let lease_body = serde_json::to_vec(&json!({
            "agent_id": "agent-01",
            "instance_id": "instance-01",
            "session_id": "session-01",
            "provider": "openai",
            "allowed_use": "session.launch",
            "ttl_seconds": 60
        }))
        .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/credentials/cred_openai_api/leases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(lease_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("sk-http-lease-secret"));
        assert!(!text.contains("plaintext"));
        let json: Value = serde_json::from_str(&text).unwrap();
        let lease_id = json["id"].as_str().unwrap().to_string();
        assert_eq!(json["credential_id"], "cred_openai_api");
        assert_eq!(json["agent_id"], "agent-01");
        assert_eq!(json["state"], "active");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v2/credentials/leases/{lease_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["state"], "revoked");

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v2/credentials/leases")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("sk-http-lease-secret"));
        assert!(!text.contains("plaintext"));
        assert!(text.contains("revoked"));
    }
}

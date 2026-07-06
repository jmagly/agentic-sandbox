//! Bootstrap enrollment consume endpoint for in-agent mTLS key enrollment.
//!
//! This endpoint is intentionally outside the operator-admin surface: the
//! bearer is the short-lived one-time bootstrap token bound to one SPIFFE id.

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::server::AppState;
use crate::bootstrap_enrollment::BootstrapTokenError;

#[derive(Debug, Deserialize)]
pub struct ConsumeBootstrapEnrollmentRequest {
    pub token: String,
    pub spiffe_id: String,
    pub csr_pem: String,
}

#[derive(Debug, Serialize)]
pub struct ConsumeBootstrapEnrollmentResponse {
    pub spiffe_id: String,
    pub certificate_pem: String,
    pub ca_pem: String,
}

pub async fn consume_bootstrap_enrollment(
    State(state): State<AppState>,
    Json(request): Json<ConsumeBootstrapEnrollmentRequest>,
) -> Response {
    if request.token.trim().is_empty() {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bootstrap.token_required",
            "Bootstrap token required",
            "token must be non-empty",
        );
    }
    if request.spiffe_id.trim().is_empty() {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bootstrap.spiffe_id_required",
            "SPIFFE id required",
            "spiffe_id must be non-empty",
        );
    }
    if request.csr_pem.trim().is_empty() {
        return problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "bootstrap.csr_required",
            "CSR required",
            "csr_pem must be non-empty",
        );
    }

    let Some(ca) = state.grpc_ca_backend.as_ref() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "bootstrap.ca_unavailable",
            "Bootstrap CA unavailable",
            "gRPC CA backend is not configured",
        );
    };
    let Some(store) = state.bootstrap_token_store.as_ref() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "bootstrap.store_unavailable",
            "Bootstrap token store unavailable",
            "bootstrap token store is not configured",
        );
    };

    let consumed = match store.consume(&request.token, &request.spiffe_id) {
        Ok(consumed) => consumed,
        Err(BootstrapTokenError::Unknown | BootstrapTokenError::Expired) => {
            return problem(
                StatusCode::UNAUTHORIZED,
                "bootstrap.token_invalid",
                "Bootstrap token invalid",
                "bootstrap token is unknown or expired",
            )
        }
        Err(BootstrapTokenError::AlreadyConsumed) => {
            return problem(
                StatusCode::CONFLICT,
                "bootstrap.token_consumed",
                "Bootstrap token already consumed",
                "bootstrap token was already consumed",
            )
        }
        Err(BootstrapTokenError::SpiffeMismatch) => {
            return problem(
                StatusCode::FORBIDDEN,
                "bootstrap.spiffe_mismatch",
                "Bootstrap token SPIFFE mismatch",
                "bootstrap token is not valid for requested SPIFFE id",
            )
        }
        Err(BootstrapTokenError::Persistence) => {
            return problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                "bootstrap.persistence_failed",
                "Bootstrap token persistence failed",
                "consumed bootstrap token state could not be persisted",
            )
        }
    };

    let issued = match ca.issue_agent_certificate_from_csr(&request.spiffe_id, &request.csr_pem) {
        Ok(issued) => issued,
        Err(err) => {
            return problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "bootstrap.csr_invalid",
                "CSR rejected",
                err.to_string(),
            )
        }
    };

    (
        StatusCode::OK,
        Json(ConsumeBootstrapEnrollmentResponse {
            spiffe_id: consumed.spiffe_id,
            certificate_pem: issued.cert_pem,
            ca_pem: ca.ca_pem().to_string(),
        }),
    )
        .into_response()
}

fn problem(status: StatusCode, code: &str, title: &str, detail: impl Into<String>) -> Response {
    let body = json!({
        "type": format!("https://agentic-sandbox.example/problems/{}", code.replace('.', "-")),
        "title": title,
        "status": status.as_u16(),
        "code": code,
        "detail": detail.into(),
    });

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/problem+json")
        .body(Body::from(serde_json::to_vec(&body).unwrap_or_default()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Router};
    use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};
    use serde_json::Value;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    fn test_state(token_dir: &std::path::Path, ca_dir: &std::path::Path) -> AppState {
        use crate::dispatch::CommandDispatcher;
        use crate::output::OutputAggregator;
        use crate::registry::AgentRegistry;

        let registry = Arc::new(AgentRegistry::new());
        AppState {
            registry: registry.clone(),
            output_agg: Arc::new(OutputAggregator::new(64)),
            dispatcher: Arc::new(CommandDispatcher::new(registry)),
            orchestrator: None,
            metrics: None,
            operation_store: Some(Arc::new(super::super::operations::OperationStore::new())),
            audit_logger: None,
            credential_broker: Arc::new(crate::credentials::CredentialBroker::new_in_memory()),
            startup_profiles: Arc::new(
                crate::startup_profiles::StartupProfileStore::new_in_memory(),
            ),
            ssh_gateway_leases: Arc::new(crate::ssh_gateway::SshGatewayLeaseStore::new_in_memory()),
            bootstrap_token_store: Some(Arc::new(
                crate::bootstrap_enrollment::BootstrapTokenStore::load_or_create(token_dir)
                    .unwrap(),
            )),
            grpc_ca_backend: Some(Arc::new(
                crate::grpc_ca_backend::LocalGrpcCaBackend::load_or_create(
                    ca_dir,
                    "sandbox-test.agentic.local",
                    crate::grpc_local_ca::LocalCaOptions::default(),
                )
                .unwrap(),
            )),
            screen_registry: None,
            hitl_store: None,
            aiwg_handle: None,
            mission_store: None,
            session_registry: None,
            transport_identity_resolver: None,
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
            idempotency_store: Arc::new(crate::http::idempotency::IdempotencyStore::new()),
        }
    }

    fn csr_for(spiffe_id: &str) -> String {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params
            .subject_alt_names
            .push(SanType::URI(spiffe_id.try_into().unwrap()));
        params.serialize_request(&key).unwrap().pem().unwrap()
    }

    #[tokio::test]
    async fn consume_endpoint_returns_signed_leaf_and_consumes_token_once() {
        let token_dir = tempfile::tempdir().unwrap();
        let ca_dir = tempfile::tempdir().unwrap();
        let state = test_state(token_dir.path(), ca_dir.path());
        let spiffe_id =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let issued_token = state
            .bootstrap_token_store
            .as_ref()
            .unwrap()
            .issue(
                "018fb9f1-3291-7a73-b261-c7de8a2af4d1",
                spiffe_id,
                Duration::from_secs(60),
            )
            .unwrap();
        let app = Router::new()
            .route(
                "/api/v1/bootstrap-enrollment/consume",
                post(super::consume_bootstrap_enrollment),
            )
            .with_state(state);
        let body = serde_json::to_vec(&json!({
            "token": issued_token.token,
            "spiffe_id": spiffe_id,
            "csr_pem": csr_for(spiffe_id),
        }))
        .unwrap();

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/bootstrap-enrollment/consume")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["spiffe_id"], spiffe_id);
        assert!(json["certificate_pem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(json["ca_pem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(!json["certificate_pem"]
            .as_str()
            .unwrap()
            .contains("PRIVATE KEY"));

        let replay = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/bootstrap-enrollment/consume")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(replay.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn consume_endpoint_rejects_token_spiffe_mismatch() {
        let token_dir = tempfile::tempdir().unwrap();
        let ca_dir = tempfile::tempdir().unwrap();
        let state = test_state(token_dir.path(), ca_dir.path());
        let token_spiffe =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1";
        let requested_spiffe =
            "spiffe://sandbox-test.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d2";
        let issued_token = state
            .bootstrap_token_store
            .as_ref()
            .unwrap()
            .issue(
                "018fb9f1-3291-7a73-b261-c7de8a2af4d1",
                token_spiffe,
                Duration::from_secs(60),
            )
            .unwrap();

        let response = super::consume_bootstrap_enrollment(
            State(state),
            Json(ConsumeBootstrapEnrollmentRequest {
                token: issued_token.token,
                spiffe_id: requested_spiffe.to_string(),
                csr_pem: csr_for(requested_spiffe),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

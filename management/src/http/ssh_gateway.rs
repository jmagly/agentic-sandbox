//! HTTP API for gateway-mediated SSH certificate leases.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use serde_json::json;

use super::server::{append_security_audit, AppState};
use crate::audit::{AuditEventType, AuditOutcome};
use crate::ssh_gateway::{
    IssueSshCertificateLeaseRequest, SshCertificateLeaseResponse, SshGatewayError,
};

#[derive(Debug, Serialize)]
struct SshLeaseListResponse {
    leases: Vec<SshCertificateLeaseResponse>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/leases", get(list_leases).post(issue_lease))
        .route("/leases/{lease_id}", get(get_lease).delete(revoke_lease))
}

async fn list_leases(State(state): State<AppState>) -> Response {
    Json(SshLeaseListResponse {
        leases: state.ssh_gateway_leases.list(),
    })
    .into_response()
}

async fn issue_lease(
    State(state): State<AppState>,
    Json(request): Json<IssueSshCertificateLeaseRequest>,
) -> Response {
    let audit_actor = request.actor.clone();
    let audit_instance = request.instance_id.clone();
    let audit_principal = request.principal.clone();
    let audit_ttl = request.ttl_seconds;

    match state.ssh_gateway_leases.issue(request) {
        Ok(response) => {
            append_security_audit(
                &state,
                AuditEventType::GatewaySshLease,
                response.actor.clone(),
                response.instance_id.clone(),
                "gateway_ssh_lease_issued",
                AuditOutcome::Success,
                json!({
                    "lease_id": response.id,
                    "principal": response.principal,
                    "access_mode": response.access_mode,
                    "ttl_seconds": response.ttl_seconds,
                    "public_key_sha256": response.public_key_sha256,
                    "certificate_key_id": response.certificate_key_id,
                    "certificate_sha256": response.certificate_sha256,
                    "expires_at": response.expires_at,
                }),
            )
            .await;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(err) => {
            append_security_audit(
                &state,
                AuditEventType::GatewaySshLease,
                audit_actor,
                audit_instance,
                "gateway_ssh_lease_denied",
                AuditOutcome::Denied,
                json!({
                    "principal": audit_principal,
                    "ttl_seconds": audit_ttl,
                    "error": err.to_string(),
                }),
            )
            .await;
            ssh_gateway_error(err)
        }
    }
}

async fn get_lease(State(state): State<AppState>, Path(lease_id): Path<String>) -> Response {
    match state.ssh_gateway_leases.get(&lease_id) {
        Ok(response) => Json(response).into_response(),
        Err(err) => ssh_gateway_error(err),
    }
}

async fn revoke_lease(State(state): State<AppState>, Path(lease_id): Path<String>) -> Response {
    match state.ssh_gateway_leases.revoke(&lease_id) {
        Ok(response) => {
            append_security_audit(
                &state,
                AuditEventType::GatewaySshLease,
                response.actor.clone(),
                response.instance_id.clone(),
                "gateway_ssh_lease_revoked",
                AuditOutcome::Success,
                json!({
                    "lease_id": response.id,
                    "principal": response.principal,
                    "access_mode": response.access_mode,
                    "ttl_seconds": response.ttl_seconds,
                    "public_key_sha256": response.public_key_sha256,
                    "revoked_at": response.revoked_at,
                }),
            )
            .await;
            Json(response).into_response()
        }
        Err(err) => ssh_gateway_error(err),
    }
}

fn ssh_gateway_error(err: SshGatewayError) -> Response {
    let status = match err {
        SshGatewayError::MissingField(_)
        | SshGatewayError::InvalidTtl
        | SshGatewayError::TtlTooLong(_)
        | SshGatewayError::UnsupportedAccessMode(_)
        | SshGatewayError::UnsupportedPrincipal(_) => StatusCode::BAD_REQUEST,
        SshGatewayError::SigningFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        SshGatewayError::LeaseNotFound(_) => StatusCode::NOT_FOUND,
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

    struct FakeSigner;

    impl crate::ssh_gateway::SshCertificateSigner for FakeSigner {
        fn sign_user_certificate(
            &self,
            lease_id: &str,
            _principal: &str,
            _public_key: &str,
            _ttl_seconds: i64,
        ) -> Result<crate::ssh_gateway::SignedSshCertificate, crate::ssh_gateway::SshGatewayError>
        {
            Ok(crate::ssh_gateway::SignedSshCertificate {
                key_id: lease_id.to_string(),
                certificate: format!("ssh-ed25519-cert-v01@openssh.com AAAAHTTPCERT {lease_id}"),
                certificate_sha256: "sha256:http-fake-cert".to_string(),
            })
        }
    }

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

    #[tokio::test]
    async fn ssh_lease_api_returns_metadata_without_key_material() {
        let app = Router::new()
            .nest("/api/v2/gateway/ssh", router())
            .with_state(test_state());
        let body = serde_json::to_vec(&json!({
            "actor": "operator@example.test",
            "instance_id": "instance-01",
            "principal": "agent",
            "access_mode": "ssh",
            "public_key": "ssh-ed25519 AAAASECRETKEY operator@example.test",
            "ttl_seconds": 60
        }))
        .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/gateway/ssh/leases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("AAAASECRETKEY"));
        assert!(!text.contains("ssh-ed25519"));
        assert!(!text.contains("public_key\""));
        let created: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(created["actor"], "operator@example.test");
        assert_eq!(created["principal"], "agent");
        assert_eq!(created["state"], "active");
        assert!(created["public_key_sha256"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v2/gateway/ssh/leases")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("AAAASECRETKEY"));
        assert!(!text.contains("ssh-ed25519"));
        assert!(text.contains("public_key_sha256"));
    }

    #[tokio::test]
    async fn ssh_lease_api_rejects_invalid_principal() {
        let app = Router::new()
            .nest("/api/v2/gateway/ssh", router())
            .with_state(test_state());
        let body = serde_json::to_vec(&json!({
            "actor": "operator@example.test",
            "instance_id": "instance-01",
            "principal": "agent;rm",
            "access_mode": "ssh",
            "public_key": "ssh-ed25519 AAAATEST operator@example.test",
            "ttl_seconds": 60
        }))
        .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/gateway/ssh/leases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ssh_lease_api_returns_signed_certificate_only_on_issue() {
        let mut state = test_state();
        state.ssh_gateway_leases = Arc::new(
            crate::ssh_gateway::SshGatewayLeaseStore::new_in_memory_with_signer(Arc::new(
                FakeSigner,
            )),
        );
        let app = Router::new()
            .nest("/api/v2/gateway/ssh", router())
            .with_state(state);
        let body = serde_json::to_vec(&json!({
            "actor": "operator@example.test",
            "instance_id": "instance-01",
            "principal": "agent",
            "access_mode": "ssh",
            "public_key": "ssh-ed25519 AAAAISSUEKEY operator@example.test",
            "ttl_seconds": 60
        }))
        .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/gateway/ssh/leases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(text.contains("AAAAHTTPCERT"));
        assert!(text.contains("certificate_sha256"));
        assert!(text.contains("certificate_key_id"));
        assert!(!text.contains("AAAAISSUEKEY"));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v2/gateway/ssh/leases")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("AAAAHTTPCERT"));
        assert!(!text.contains("\"certificate\":"));
        assert!(text.contains("certificate_sha256"));
    }

    #[tokio::test]
    async fn ssh_lease_api_audits_metadata_without_key_material() {
        let temp_dir = tempfile::tempdir().unwrap();
        let audit_dir = temp_dir.path().join("audit");
        let logger = crate::audit::AuditLogger::new(crate::audit::AuditConfig {
            log_dir: audit_dir.clone(),
            enable_integrity_chain: false,
            ..Default::default()
        })
        .await
        .unwrap();
        let mut state = test_state();
        state.audit_logger = Some(Arc::new(logger));
        let app = Router::new()
            .nest("/api/v2/gateway/ssh", router())
            .with_state(state);
        let body = serde_json::to_vec(&json!({
            "actor": "operator@example.test",
            "instance_id": "instance-01",
            "principal": "agent",
            "access_mode": "ssh",
            "public_key": "ssh-ed25519 AAAAAUDITKEY operator@example.test",
            "ttl_seconds": 60
        }))
        .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v2/gateway/ssh/leases")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let audit_log = std::fs::read_to_string(audit_dir.join(format!("audit-{today}.jsonl")))
            .expect("audit log should be written");
        assert!(audit_log.contains("gateway_ssh_lease"));
        assert!(audit_log.contains("gateway_ssh_lease_issued"));
        assert!(audit_log.contains("operator@example.test"));
        assert!(audit_log.contains("instance-01"));
        assert!(audit_log.contains("sha256:"));
        assert!(!audit_log.contains("AAAAAUDITKEY"));
        assert!(!audit_log.contains("ssh-ed25519"));
        assert!(!audit_log.contains("public_key\""));
    }
}

//! HTTP API for workload credential metadata.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::get,
    Extension, Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::operator_auth::{OperatorIdentity, OperatorRole, RequireAdmin};
use super::server::{append_security_audit, AppState};
use crate::audit::{AuditEventType, AuditOutcome, AuditQueryFilter};
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

#[derive(Debug, Serialize)]
struct AccessAuthorityResponse {
    schema_version: &'static str,
    mode: &'static str,
    actor: Option<String>,
    role: Option<&'static str>,
    permissions: Vec<&'static str>,
}

#[derive(Debug, Default, Deserialize)]
struct AccessAuditQuery {
    date: Option<String>,
}

#[derive(Debug, Serialize)]
struct AccessAuditEvent {
    id: String,
    sequence: u64,
    timestamp: chrono::DateTime<Utc>,
    event_type: String,
    actor: String,
    resource: String,
    correlation_id: Option<String>,
    action: String,
    outcome: String,
    trace_id: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_credentials).post(create_credential))
        .route("/authority", get(get_access_authority))
        .route("/audit", get(list_access_audit))
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

fn request_actor(identity: Option<&Extension<OperatorIdentity>>) -> String {
    identity
        .map(|Extension(identity)| identity.actor.clone())
        .unwrap_or_else(|| "local-trusted-network".to_string())
}

async fn append_access_audit(
    state: &AppState,
    identity: Option<&Extension<OperatorIdentity>>,
    event_type: AuditEventType,
    resource: impl Into<String>,
    action: impl Into<String>,
    outcome: AuditOutcome,
) {
    append_security_audit(
        state,
        event_type,
        request_actor(identity),
        resource,
        action,
        outcome,
        serde_json::json!({"metadata_only": true}),
    )
    .await;
}

async fn get_access_authority(
    State(state): State<AppState>,
    role: Option<Extension<OperatorRole>>,
    identity: Option<Extension<OperatorIdentity>>,
) -> Response {
    let explicitly_configured = state.operator_auth.is_some()
        || !state.mtls_config.is_empty()
        || state.unix_peer_creds_config.is_explicit();
    let effective_role = role
        .map(|Extension(role)| role)
        .or_else(|| (!explicitly_configured).then_some(OperatorRole::Admin));
    let mut permissions = Vec::new();
    if effective_role.is_some() {
        permissions.extend([
            "credential.read",
            "credential_lease.read",
            "access_audit.read",
        ]);
    }
    if effective_role == Some(OperatorRole::Admin) {
        permissions.extend([
            "credential.create",
            "credential.update",
            "credential.delete",
            "credential_lease.issue",
            "credential_lease.revoke",
        ]);
    } else if effective_role == Some(OperatorRole::Operator) {
        permissions.extend(["credential_lease.issue", "credential_lease.revoke"]);
    }
    if identity.is_some() && effective_role.is_some() {
        permissions.extend(["ssh_lease.read", "ssh_lease.issue", "ssh_lease.revoke"]);
    }
    Json(AccessAuthorityResponse {
        schema_version: "management.access-authority/v1",
        mode: if identity.is_some() {
            "authenticated_operator"
        } else if explicitly_configured {
            "unresolved"
        } else {
            "trusted_local_listener"
        },
        actor: identity
            .as_ref()
            .map(|Extension(identity)| identity.actor.clone())
            .or_else(|| (!explicitly_configured).then(|| "local-trusted-network".to_string())),
        role: effective_role.map(OperatorRole::as_str),
        permissions,
    })
    .into_response()
}

async fn list_access_audit(
    State(state): State<AppState>,
    Query(query): Query<AccessAuditQuery>,
) -> Response {
    let date = query
        .date
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    if chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "date must use YYYY-MM-DD"})),
        )
            .into_response();
    }
    let Some(logger) = state.audit_logger.as_ref() else {
        return Json(serde_json::json!({
            "available": false,
            "reason": "security audit logger is not configured",
            "date": date,
            "events": [],
        }))
        .into_response();
    };
    let events = match logger
        .query(
            &date,
            Some(AuditQueryFilter {
                limit: Some(1000),
                ..Default::default()
            }),
        )
        .await
    {
        Ok(events) => events,
        Err(crate::audit::AuditError::LogNotFound(_)) => Vec::new(),
        Err(error) => {
            tracing::warn!(error = %error, audit_date = %date, "access audit query unavailable");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "audit query unavailable"})),
            )
                .into_response();
        }
    };
    let mut projected: Vec<_> = events
        .into_iter()
        .filter(|event| {
            matches!(
                &event.event_type,
                AuditEventType::SecretAccess
                    | AuditEventType::SecretRotation
                    | AuditEventType::GatewaySshLease
            ) || (event.event_type == AuditEventType::ConfigChange
                && event.action.starts_with("credential_"))
        })
        .map(|event| AccessAuditEvent {
            id: event.id,
            sequence: event.sequence,
            timestamp: event.timestamp,
            event_type: event.event_type.to_string(),
            actor: event.actor,
            resource: event.resource,
            correlation_id: event
                .details
                .get("lease_id")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            action: event.action,
            outcome: event.outcome.to_string(),
            trace_id: event.trace_id,
        })
        .collect();
    projected.sort_by(|left, right| right.sequence.cmp(&left.sequence));
    projected.truncate(200);
    Json(serde_json::json!({
        "available": true,
        "date": date,
        "events": projected,
    }))
    .into_response()
}

async fn list_credentials(State(state): State<AppState>) -> Response {
    Json(CredentialListResponse {
        credentials: state.credential_broker.list(),
    })
    .into_response()
}

async fn create_credential(
    State(state): State<AppState>,
    identity: Option<Extension<OperatorIdentity>>,
    _admin: RequireAdmin,
    Json(request): Json<UpsertCredentialRequest>,
) -> Response {
    let id = request.id.clone();
    match state.credential_broker.create(request) {
        Ok(response) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::ConfigChange,
                id,
                "credential_created",
                AuditOutcome::Success,
            )
            .await;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(err) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::ConfigChange,
                id,
                "credential_create_denied",
                AuditOutcome::Denied,
            )
            .await;
            credential_error(err)
        }
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

async fn revoke_lease(
    State(state): State<AppState>,
    identity: Option<Extension<OperatorIdentity>>,
    Path(lease_id): Path<String>,
) -> Response {
    match state.credential_broker.revoke_lease(&lease_id) {
        Ok(response) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::SecretAccess,
                lease_id,
                "credential_lease_revoked",
                AuditOutcome::Success,
            )
            .await;
            Json(response).into_response()
        }
        Err(err) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::SecretAccess,
                lease_id,
                "credential_lease_revoke_denied",
                AuditOutcome::Denied,
            )
            .await;
            credential_error(err)
        }
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
    identity: Option<Extension<OperatorIdentity>>,
    Path(id): Path<String>,
    Json(request): Json<IssueCredentialLeaseRequest>,
) -> Response {
    match state.credential_broker.issue_lease(&id, request) {
        Ok(response) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::SecretAccess,
                response.id.clone(),
                "credential_lease_issued",
                AuditOutcome::Success,
            )
            .await;
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(err) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::SecretAccess,
                id,
                "credential_lease_issue_denied",
                AuditOutcome::Denied,
            )
            .await;
            credential_error(err)
        }
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
    identity: Option<Extension<OperatorIdentity>>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
    Json(request): Json<UpsertCredentialRequest>,
) -> Response {
    match state.credential_broker.update(&id, request) {
        Ok(response) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::ConfigChange,
                id,
                "credential_updated",
                AuditOutcome::Success,
            )
            .await;
            Json(response).into_response()
        }
        Err(err) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::ConfigChange,
                id,
                "credential_update_denied",
                AuditOutcome::Denied,
            )
            .await;
            credential_error(err)
        }
    }
}

async fn delete_credential(
    State(state): State<AppState>,
    identity: Option<Extension<OperatorIdentity>>,
    _admin: RequireAdmin,
    Path(id): Path<String>,
) -> Response {
    match state.credential_broker.delete(&id) {
        Ok(()) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::ConfigChange,
                id,
                "credential_deleted",
                AuditOutcome::Success,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => {
            append_access_audit(
                &state,
                identity.as_ref(),
                AuditEventType::ConfigChange,
                id,
                "credential_delete_denied",
                AuditOutcome::Denied,
            )
            .await;
            credential_error(err)
        }
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
        | CredentialError::UnsupportedBackend(_)
        | CredentialError::ProxyDenied(_)
        | CredentialError::ProxyPolicyMissing(_) => StatusCode::FORBIDDEN,
        CredentialError::ProxyRateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
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
            activity_store: crate::activity::ActivityStore::in_memory().unwrap(),
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
            transport_identity_resolver: None,
            agentshare_root: None,
            tasks_root: None,
            operator_auth: None,
            mtls_config: super::super::operator_auth::MtlsConfig::default(),
            unix_peer_creds_config: super::super::operator_auth::UnixPeerCredsConfig::default(),
            executor_instance_registry: None,
            executor_signing_keys_dir: None,
            executor_idempotency: None,
            executor_task_store: None,
            host_runtime_supervisor: None,
            v1_counter: None,
            idempotency_store: Arc::new(crate::http::idempotency::IdempotencyStore::new()),
            mcp_config: None,
            management_ws_endpoint: None,
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

    #[tokio::test]
    async fn trusted_local_authority_does_not_claim_ssh_identity() {
        let app = Router::new()
            .nest("/api/v2/credentials", router())
            .with_state(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v2/credentials/authority")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["schema_version"], "management.access-authority/v1");
        assert_eq!(body["mode"], "trusted_local_listener");
        assert_eq!(body["role"], "admin");
        let permissions = body["permissions"].as_array().unwrap();
        assert!(permissions
            .iter()
            .any(|value| value.as_str() == Some("credential_lease.issue")));
        assert!(!permissions
            .iter()
            .any(|value| value.as_str() == Some("ssh_lease.issue")));
    }

    #[tokio::test]
    async fn access_audit_reports_explicit_unavailable_state() {
        let app = Router::new()
            .nest("/api/v2/credentials", router())
            .with_state(test_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v2/credentials/audit?date=2026-09-03")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["available"], false);
        assert_eq!(body["events"], json!([]));
    }

    #[tokio::test]
    async fn access_audit_projection_omits_event_details() {
        let temp = tempfile::tempdir().unwrap();
        let logger = Arc::new(
            crate::audit::AuditLogger::new(crate::audit::AuditConfig {
                log_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        logger
            .log(
                crate::audit::AuditEvent::new(
                    AuditEventType::SecretAccess,
                    "operator-1",
                    "lease-1",
                    "credential_lease_issued",
                    AuditOutcome::Success,
                )
                .with_details(json!({
                    "lease_id": "lease-1",
                    "secret": "sk-audit-must-not-render"
                })),
            )
            .await
            .unwrap();
        let mut state = test_state();
        state.audit_logger = Some(logger);
        let date = Utc::now().format("%Y-%m-%d");
        let app = Router::new()
            .nest("/api/v2/credentials", router())
            .with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v2/credentials/audit?date={date}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("sk-audit-must-not-render"));
        assert!(!text.contains("details"));
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["events"][0]["correlation_id"], "lease-1");
    }
}

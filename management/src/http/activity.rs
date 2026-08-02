//! HTTP bindings for the metadata-only activity event store.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde_json::json;

use super::operator_auth::{OperatorIdentity, RequireAdmin};
use super::server::AppState;
use crate::activity::{ActivityError, ActivityQuery, IngestBatch, IngestScope};
use crate::audit::{AuditEventType, AuditOutcome};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ingest", post(ingest))
        .route("/events", get(query_events))
        .route("/timeline", get(query_events))
        .route("/coverage", get(query_coverage))
        .route("/export", post(export_events))
}

async fn ingest(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    headers: HeaderMap,
    Json(batch): Json<IngestBatch>,
) -> Response {
    let scope = match scope_from_headers(&headers, true) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    match state.activity_store.ingest(&scope, batch) {
        Ok(ack) => (StatusCode::OK, Json(ack)).into_response(),
        Err(error) => activity_error(error),
    }
}

async fn query_events(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    identity: Option<Extension<OperatorIdentity>>,
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> Response {
    let scope = match scope_from_headers(&headers, false) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let actor_id = actor_from_identity(identity.as_ref());
    let resource = scope_resource(&scope);
    match state.activity_store.query(&scope, &query) {
        Ok(result) => {
            super::server::append_security_audit(
                &state,
                AuditEventType::ApiAccess,
                actor_id,
                resource,
                "activity_query",
                AuditOutcome::Success,
                json!({"event_count": result.events.len(), "complete": result.completeness.complete}),
            )
            .await;
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(error) => {
            super::server::append_security_audit(
                &state,
                AuditEventType::ApiAccess,
                actor_id,
                resource,
                "activity_query",
                AuditOutcome::Failure,
                json!({"error_class": activity_error_class(&error)}),
            )
            .await;
            activity_error(error)
        }
    }
}

async fn query_coverage(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    identity: Option<Extension<OperatorIdentity>>,
    headers: HeaderMap,
) -> Response {
    let scope = match scope_from_headers(&headers, false) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let actor_id = actor_from_identity(identity.as_ref());
    let resource = scope_resource(&scope);
    match state
        .activity_store
        .query(&scope, &ActivityQuery::default())
    {
        Ok(result) => {
            super::server::append_security_audit(
                &state,
                AuditEventType::ApiAccess,
                actor_id,
                resource,
                "activity_coverage_query",
                AuditOutcome::Success,
                json!({"complete": result.completeness.complete}),
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({
                "schema_version": result.schema_version,
                "coverage": result.coverage,
                "completeness": result.completeness,
                })),
            )
                .into_response()
        }
        Err(error) => {
            super::server::append_security_audit(
                &state,
                AuditEventType::ApiAccess,
                actor_id,
                resource,
                "activity_coverage_query",
                AuditOutcome::Failure,
                json!({"error_class": activity_error_class(&error)}),
            )
            .await;
            activity_error(error)
        }
    }
}

async fn export_events(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    identity: Option<Extension<OperatorIdentity>>,
    headers: HeaderMap,
    Json(query): Json<ActivityQuery>,
) -> Response {
    let scope = match scope_from_headers(&headers, false) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let actor_id = actor_from_identity(identity.as_ref());
    let resource = scope_resource(&scope);
    match state.activity_store.export(&scope, &query, &actor_id) {
        Ok(export) => {
            super::server::append_security_audit(
                &state,
                AuditEventType::ApiAccess,
                actor_id.clone(),
                resource,
                "activity_export",
                AuditOutcome::Success,
                json!({
                    "batch_id": export.manifest.batch_id,
                    "event_count": export.manifest.event_count,
                    "merkle_root": export.manifest.merkle_root,
                    "key_id": export.manifest.key_id,
                }),
            )
            .await;
            (StatusCode::OK, Json(export)).into_response()
        }
        Err(error) => {
            super::server::append_security_audit(
                &state,
                AuditEventType::ApiAccess,
                actor_id,
                resource,
                "activity_export",
                AuditOutcome::Failure,
                json!({"error_class": activity_error_class(&error)}),
            )
            .await;
            activity_error(error)
        }
    }
}

fn actor_from_identity(identity: Option<&Extension<OperatorIdentity>>) -> String {
    identity
        .map(|Extension(identity)| identity.actor.clone())
        .unwrap_or_else(|| "trusted-network-admin".to_owned())
}

fn scope_resource(scope: &IngestScope) -> String {
    format!(
        "{}/{}/{}/{}",
        scope.tenant_id, scope.host_id, scope.instance_id, scope.agent_id
    )
}

fn activity_error_class(error: &ActivityError) -> &'static str {
    match error {
        ActivityError::Invalid(_) => "invalid",
        ActivityError::Scope(_) => "scope",
        ActivityError::Storage(_) => "storage",
        ActivityError::Corrupt(_) => "corrupt",
        ActivityError::Unavailable(_) => "unavailable",
    }
}

fn scope_from_headers(
    headers: &HeaderMap,
    require_collector: bool,
) -> Result<IngestScope, Response> {
    fn required(headers: &HeaderMap, name: &'static str) -> Result<String, Response> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= 255)
            .map(str::to_owned)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("missing or invalid {name} header")})),
                )
                    .into_response()
            })
    }

    Ok(IngestScope {
        tenant_id: required(headers, "x-agentic-tenant-id")?,
        host_id: required(headers, "x-agentic-host-id")?,
        instance_id: required(headers, "x-agentic-instance-id")?,
        agent_id: required(headers, "x-agentic-agent-id")?,
        collector_id: if require_collector {
            required(headers, "x-agentic-collector-id")?
        } else {
            headers
                .get("x-agentic-collector-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("*")
                .to_owned()
        },
    })
}

fn activity_error(error: ActivityError) -> Response {
    let status = match error {
        ActivityError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ActivityError::Scope(_) => StatusCode::FORBIDDEN,
        ActivityError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        ActivityError::Storage(_) | ActivityError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error": error.to_string()}))).into_response()
}

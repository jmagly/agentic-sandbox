//! HTTP bindings for the metadata-only activity event store.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use super::operator_auth::RequireAdmin;
use super::server::AppState;
use crate::activity::{ActivityError, ActivityQuery, IngestBatch, IngestScope};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/ingest", post(ingest))
        .route("/events", get(query_events))
        .route("/coverage", get(query_coverage))
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
    headers: HeaderMap,
    Query(query): Query<ActivityQuery>,
) -> Response {
    let scope = match scope_from_headers(&headers, false) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    match state.activity_store.query(&scope, &query) {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(error) => activity_error(error),
    }
}

async fn query_coverage(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    headers: HeaderMap,
) -> Response {
    let scope = match scope_from_headers(&headers, false) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    match state
        .activity_store
        .query(&scope, &ActivityQuery::default())
    {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({
                "schema_version": result.schema_version,
                "coverage": result.coverage,
            })),
        )
            .into_response(),
        Err(error) => activity_error(error),
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
        ActivityError::Storage(_) | ActivityError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error": error.to_string()}))).into_response()
}

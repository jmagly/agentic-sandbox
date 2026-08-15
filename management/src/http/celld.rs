use crate::celld::{
    plan_upgrade, preflight_bucket, BucketPreflightEvidence, CellCommand, CelldClient, CelldConfig,
    CelldFleetManifest, CelldStatus, WorkerBundleManifest,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use super::operator_auth::RequireAdmin;

#[derive(Clone)]
struct CelldApiState {
    status: CelldStatus,
    client: Option<CelldClient>,
    configuration_error: Option<String>,
}

pub fn router_from_env() -> Router {
    let state = match CelldConfig::from_env() {
        Ok(config) => {
            let status = config.status();
            match CelldClient::new(config) {
                Ok(client) => CelldApiState {
                    status,
                    client: Some(client),
                    configuration_error: None,
                },
                Err(crate::celld::client::ClientError::Disabled) => CelldApiState {
                    status,
                    client: None,
                    configuration_error: None,
                },
                Err(error) => CelldApiState {
                    status,
                    client: None,
                    configuration_error: Some(error.to_string()),
                },
            }
        }
        Err(error) => CelldApiState {
            status: CelldStatus {
                enabled: true,
                configured: false,
                endpoint: None,
                celld_version: crate::celld::client::PINNED_CELLD_VERSION.into(),
                celld_commit: crate::celld::client::PINNED_CELLD_COMMIT.into(),
                protocol_version: "celld-internal-v1".into(),
                adapter_version: env!("CARGO_PKG_VERSION").into(),
                security_posture: "configuration rejected; no Celld traffic permitted".into(),
                unavailable_code: Some("celld.configuration_invalid".into()),
            },
            client: None,
            configuration_error: Some(error.to_string()),
        },
    };
    router(state)
}

fn router(state: CelldApiState) -> Router {
    Router::new()
        .route("/api/v2/celld/status", get(status))
        .route("/api/v2/celld/cells/{instance_id}", get(get_cell))
        .route("/api/v2/celld/cells/{instance_id}/commands", post(command))
        .route(
            "/api/v2/celld/cells/{instance_id}/reconcile",
            post(reconcile),
        )
        .route("/api/v2/celld/bundles/validate", post(validate_bundle))
        .route("/api/v2/celld/fleets/validate", post(validate_fleet))
        .route("/api/v2/celld/fleets/preflight", post(preflight))
        .route("/api/v2/celld/fleets/plan-upgrade", post(upgrade))
        .with_state(state)
}

async fn status(State(state): State<CelldApiState>) -> Json<serde_json::Value> {
    Json(json!({"status":state.status,"configuration_error":state.configuration_error}))
}

#[derive(Deserialize)]
struct GenerationQuery {
    generation: u64,
}
async fn get_cell(
    State(state): State<CelldApiState>,
    Path(instance_id): Path<String>,
    Query(query): Query<GenerationQuery>,
) -> Response {
    match require_client(&state) {
        Ok(client) => respond(client.get_cell(&instance_id, query.generation).await),
        Err(response) => response,
    }
}

async fn command(
    State(state): State<CelldApiState>,
    Path(instance_id): Path<String>,
    _admin: RequireAdmin,
    Json(command): Json<CellCommand>,
) -> Response {
    if command.instance_id != instance_id {
        return error(
            StatusCode::BAD_REQUEST,
            "celld.instance_mismatch",
            "path and command instance ids differ",
        );
    }
    match require_client(&state) {
        Ok(client) => respond(client.command(&command).await),
        Err(response) => response,
    }
}

#[derive(Deserialize)]
struct ReconcileRequest {
    management_generation: u64,
}
async fn reconcile(
    State(state): State<CelldApiState>,
    Path(instance_id): Path<String>,
    _admin: RequireAdmin,
    Json(request): Json<ReconcileRequest>,
) -> Response {
    match require_client(&state) {
        Ok(client) => respond(
            client
                .reconcile(&instance_id, request.management_generation)
                .await,
        ),
        Err(response) => response,
    }
}

async fn validate_bundle(Json(manifest): Json<WorkerBundleManifest>) -> Response {
    match manifest.validate() {
        Ok(()) => (
            StatusCode::OK,
            Json(
                json!({"valid":true,"runtime":"worker-celld","capabilities":manifest.capabilities}),
            ),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
async fn validate_fleet(Json(manifest): Json<CelldFleetManifest>) -> Response {
    match manifest.validate() {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"valid":true,"fleet_id":manifest.fleet_id})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
async fn preflight(Json(evidence): Json<BucketPreflightEvidence>) -> Response {
    match preflight_bucket(&evidence) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({"qualified":true,"evidence":evidence})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}
#[derive(Deserialize)]
struct UpgradeRequest {
    manifest: CelldFleetManifest,
    from: String,
    to: String,
}
async fn upgrade(Json(request): Json<UpgradeRequest>) -> Response {
    match plan_upgrade(&request.manifest, &request.from, &request.to) {
        Ok(plan) => (StatusCode::OK, Json(json!({"qualified":true,"plan":plan}))).into_response(),
        Err(error) => error_response(error),
    }
}

fn require_client(state: &CelldApiState) -> Result<&CelldClient, Response> {
    state.client.as_ref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            state
                .status
                .unavailable_code
                .as_deref()
                .unwrap_or("celld.unavailable"),
            state
                .configuration_error
                .as_deref()
                .unwrap_or("Celld is disabled or unavailable"),
        )
    })
}
fn respond<T: serde::Serialize>(result: Result<T, crate::celld::client::ClientError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error_value) => {
            let detail = public_client_error(&error_value);
            error(StatusCode::BAD_GATEWAY, "celld.upstream_failure", &detail)
        }
    }
}

fn public_client_error(error: &crate::celld::client::ClientError) -> String {
    match error {
        crate::celld::client::ClientError::Response { status, .. } => {
            format!("Celld returned HTTP {status}; response body withheld")
        }
        crate::celld::client::ClientError::Disabled => "Celld is disabled".into(),
        _ => "Celld upstream request failed; inspect redacted service diagnostics".into(),
    }
}
fn error_response(error_value: crate::celld::ValidationError) -> Response {
    error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "celld.validation_failed",
        &error_value.to_string(),
    )
}
fn error(status: StatusCode, code: &str, detail: &str) -> Response {
    (status, Json(json!({"error":{"code":code,"detail":detail}}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        celld::{client::ClientError, CellAction},
        http::operator_auth::OperatorRole,
    };
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    fn disabled_state() -> CelldApiState {
        CelldApiState {
            status: CelldStatus {
                enabled: false,
                configured: false,
                endpoint: None,
                celld_version: crate::celld::client::PINNED_CELLD_VERSION.into(),
                celld_commit: crate::celld::client::PINNED_CELLD_COMMIT.into(),
                protocol_version: "celld-internal-v1".into(),
                adapter_version: "test".into(),
                security_posture: "test".into(),
                unavailable_code: Some("celld.disabled".into()),
            },
            client: None,
            configuration_error: None,
        }
    }

    fn operator_request(path: &str, body: serde_json::Value) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        request.extensions_mut().insert(OperatorRole::Operator);
        request
    }

    #[tokio::test]
    async fn command_and_reconcile_require_admin_role() {
        let app = router(disabled_state());
        let command =
            CellCommand::new("op-admin", "instance-a", 1, CellAction::Observe, json!({})).unwrap();
        let command_response = app
            .clone()
            .oneshot(operator_request(
                "/api/v2/celld/cells/instance-a/commands",
                serde_json::to_value(command).unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(command_response.status(), StatusCode::FORBIDDEN);

        let reconcile_response = app
            .oneshot(operator_request(
                "/api/v2/celld/cells/instance-a/reconcile",
                json!({"management_generation":1}),
            ))
            .await
            .unwrap();
        assert_eq!(reconcile_response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn public_upstream_error_never_echoes_raw_body() {
        let secret = "AWS_SECRET_ACCESS_KEY=should-never-escape";
        let detail = public_client_error(&ClientError::Response {
            status: 502,
            body: secret.repeat(1_000),
        });
        assert_eq!(detail, "Celld returned HTTP 502; response body withheld");
        assert!(!detail.contains(secret));
        assert!(detail.len() < 128);
    }
}

//! Neutral fleet workload management projection (#736/#737).
//!
//! AIWG/Cockpit is the first management-plane consumer, but these routes use
//! only `agentic-orchestration/v1` records and remain usable by other
//! orchestrators. The executor TaskStore supplies durable idempotency and
//! monotonic revision checks across management-server restarts.

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use agentic_sandbox_executor::store::task_store::{
    ArtifactRow, FleetDispatchOutcome, FleetObservationOutcome, FleetWorkloadRow, TaskRow,
    TaskState, TaskStore,
};

use super::operations::{Operation, OperationState, OperationType};
use super::operator_auth::RequireAdmin;
use super::server::AppState;

const API_VERSION: &str = "agentic-orchestration/v1";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workloads", post(dispatch).get(inventory))
        .route("/workloads/{child_id}", get(get_workload))
        .route("/workloads/{child_id}/observations", post(observe))
        .route("/reconcile", post(reconcile))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationRequest {
    expected_revision: u64,
    status: Value,
    runtime_identity: Option<RuntimeIdentityRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeIdentityRequest {
    session_id: Option<String>,
    task_id: Option<String>,
    command_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconcileRequest {
    before_revision: u64,
    child_ids: Vec<String>,
}

async fn dispatch(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(mut record): Json<Value>,
) -> Response {
    let Some(store) = state.executor_task_store.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fleet.store_unavailable",
            "executor store is not mounted",
        );
    };
    if let Err(message) = validate_dispatch_record(&record) {
        return error(StatusCode::BAD_REQUEST, "fleet.invalid_record", &message);
    }
    let request_hash = match dispatch_request_hash(&record) {
        Ok(hash) => hash,
        Err(message) => return error(StatusCode::BAD_REQUEST, "fleet.invalid_record", &message),
    };
    let child_id = record["lineage"]["child_id"].as_str().unwrap().to_string();
    let idempotency_key = record["lineage"]["idempotency_key"]
        .as_str()
        .unwrap()
        .to_string();
    let target_id = record["lineage"]["target_id"].as_str().unwrap().to_string();
    let executor_id = record["lineage"]["executor_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workload_kind = record["kind"].as_str().unwrap().to_string();
    let now = Utc::now();
    record["status"]["observed_state"] = json!("admitted");
    record["status"]["revision"] = json!(1);
    record["status"]["last_seen"] = json!(now.to_rfc3339());
    let row = FleetWorkloadRow {
        child_id,
        idempotency_key,
        request_hash,
        target_id,
        executor_id,
        workload_kind,
        observed_state: "admitted".into(),
        revision: 1,
        last_seen: now,
        record_json: record,
        created_at: now,
        updated_at: now,
    };

    match store.fleet_dispatch(&row) {
        Ok(FleetDispatchOutcome::Inserted(row)) => (
            StatusCode::ACCEPTED,
            Json(json!({"replayed": false, "workload": row.record_json})),
        )
            .into_response(),
        Ok(FleetDispatchOutcome::Replay(row)) => (
            StatusCode::OK,
            Json(json!({"replayed": true, "workload": row.record_json})),
        )
            .into_response(),
        Ok(FleetDispatchOutcome::Collision) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "fleet.idempotency_collision",
            "idempotency key or child id was reused with a different payload",
        ),
        Err(cause) => internal("fleet.dispatch_failed", cause),
    }
}

async fn get_workload(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(child_id): Path<String>,
) -> Response {
    let Some(store) = state.executor_task_store.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fleet.store_unavailable",
            "executor store is not mounted",
        );
    };
    match store.get_fleet_workload(&child_id) {
        Ok(Some(row)) => {
            let row = refresh_from_task_store(store, row);
            (StatusCode::OK, Json(row.record_json)).into_response()
        }
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "fleet.workload_not_found",
            "workload was not found",
        ),
        Err(cause) => internal("fleet.inventory_failed", cause),
    }
}

async fn inventory(State(state): State<AppState>, _admin: RequireAdmin) -> Response {
    let Some(store) = state.executor_task_store.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fleet.store_unavailable",
            "executor store is not mounted",
        );
    };
    match store.list_fleet_workloads() {
        Ok(rows) => {
            let rows = rows
                .into_iter()
                .map(|row| refresh_from_task_store(store, row))
                .collect::<Vec<_>>();
            let inventory_revision = inventory_revision(&rows);
            let records: Vec<Value> = rows.into_iter().map(|row| row.record_json).collect();
            (
                StatusCode::OK,
                Json(json!({
                    "document_type": "inventory",
                    "api_version": API_VERSION,
                    "inventory_revision": inventory_revision,
                    "generated_at": Utc::now().to_rfc3339(),
                    "records": records,
                })),
            )
                .into_response()
        }
        Err(cause) => internal("fleet.inventory_failed", cause),
    }
}

fn refresh_from_task_store(store: &TaskStore, current: FleetWorkloadRow) -> FleetWorkloadRow {
    let Some(task_id) = current
        .record_json
        .pointer("/lineage/task_id")
        .and_then(Value::as_str)
    else {
        return current;
    };
    let task = match store.get_task(task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return current,
        Err(cause) => {
            tracing::warn!(error = %cause, task_id, "fleet projection could not load bound task");
            return current;
        }
    };
    let artifacts = match store.list_artifacts(task_id) {
        Ok(artifacts) => artifacts,
        Err(cause) => {
            tracing::warn!(error = %cause, task_id, "fleet projection could not load task artifacts");
            Vec::new()
        }
    };
    let Some(next) = project_task_observation(&current, &task, &artifacts) else {
        return current;
    };
    match store.observe_fleet_workload(&current.child_id, current.revision, &next) {
        Ok(FleetObservationOutcome::Updated(row)) => row,
        Ok(FleetObservationOutcome::Stale { .. }) => store
            .get_fleet_workload(&current.child_id)
            .ok()
            .flatten()
            .unwrap_or(current),
        Ok(FleetObservationOutcome::Missing) => current,
        Err(cause) => {
            tracing::warn!(error = %cause, child_id = %current.child_id, "fleet projection could not persist task observation");
            current
        }
    }
}

fn project_task_observation(
    current: &FleetWorkloadRow,
    task: &TaskRow,
    task_artifacts: &[ArtifactRow],
) -> Option<FleetWorkloadRow> {
    if matches!(
        current.observed_state.as_str(),
        "succeeded" | "failed" | "cancelled" | "timed-out"
    ) {
        return None;
    }
    let kind = current.workload_kind.as_str();
    let (observed_state, health, exit_classification, error_code) = match task.state {
        TaskState::Submitted => (
            "starting",
            (kind == "daemon").then_some("unknown"),
            None,
            None,
        ),
        TaskState::Working => match kind {
            "daemon" => ("healthy", Some("healthy"), None, None),
            "scheduled-collector" => ("catching-up", None, None, None),
            _ => ("running", None, None, None),
        },
        TaskState::Completed => match kind {
            "persistent-agent" => ("retained", None, Some("success"), None),
            "daemon" => (
                "operator-review-required",
                Some("unknown"),
                Some("unknown"),
                Some("fleet.daemon_task_exited"),
            ),
            "scheduled-collector" => ("scheduled", None, Some("success"), None),
            _ => ("succeeded", None, Some("success"), None),
        },
        TaskState::Failed | TaskState::Rejected => (
            "failed",
            (kind == "daemon").then_some("unhealthy"),
            Some("failure"),
            Some("fleet.task_failed"),
        ),
        TaskState::Canceled => (
            "cancelled",
            (kind == "daemon").then_some("unknown"),
            Some("cancelled"),
            None,
        ),
        TaskState::InputRequired => (
            "blocked",
            (kind == "daemon").then_some("unknown"),
            None,
            None,
        ),
        TaskState::AuthRequired => (
            "blocked",
            (kind == "daemon").then_some("unknown"),
            None,
            Some("fleet.task_auth_required"),
        ),
    };

    let mut artifacts = task_artifacts
        .iter()
        .map(|artifact| {
            json!({
                "kind": "log",
                "uri": format!("sandbox://tasks/{}/artifacts/{}", task.task_id, artifact.artifact_id),
                "sha256": canonical_hash(&artifact.artifact_json).unwrap_or_else(|_| "0".repeat(64)),
            })
        })
        .collect::<Vec<_>>();
    if task.state == TaskState::Completed {
        artifacts.push(json!({
            "kind": "result",
            "uri": format!("sandbox://tasks/{}/status", task.task_id),
            "sha256": canonical_hash(&task.status_json).unwrap_or_else(|_| "0".repeat(64)),
        }));
    }

    let last_seen = task_artifacts
        .iter()
        .map(|artifact| artifact.created_at)
        .fold(task.updated_at, std::cmp::max);
    let mut status = json!({
        "observed_state": observed_state,
        "revision": current.revision + 1,
        "last_seen": last_seen.to_rfc3339(),
        "artifacts": artifacts,
    });
    if let Some(health) = health {
        status["health"] = json!(health);
    }
    if task.state == TaskState::InputRequired {
        status["backpressure"] = json!({"reason": "approval", "retryable": false});
    } else if task.state == TaskState::AuthRequired {
        status["backpressure"] = json!({"reason": "policy", "retryable": false});
    }
    if let Some(classification) = exit_classification {
        status["exit_classification"] = json!(classification);
    }
    if let Some(code) = error_code {
        status["error_code"] = json!(code);
    }

    let current_status = &current.record_json["status"];
    let same_projection = current_status["observed_state"] == status["observed_state"]
        && current_status["health"] == status["health"]
        && current_status["backpressure"] == status["backpressure"]
        && current_status["artifacts"] == status["artifacts"]
        && current_status["exit_classification"] == status["exit_classification"]
        && current_status["error_code"] == status["error_code"];
    if same_projection {
        return None;
    }

    let mut next = current.clone();
    next.observed_state = observed_state.into();
    next.revision += 1;
    next.last_seen = last_seen;
    next.updated_at = Utc::now();
    next.record_json["status"] = status;
    if next.record_json["lineage"]["command_id"].is_null() {
        if let Some(command_id) = task_artifacts
            .iter()
            .find_map(|artifact| artifact.artifact_json["command_id"].as_str())
        {
            next.record_json["lineage"]["command_id"] = json!(command_id);
        }
    }
    Some(next)
}

async fn observe(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Path(child_id): Path<String>,
    Json(request): Json<ObservationRequest>,
) -> Response {
    let Some(store) = state.executor_task_store.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fleet.store_unavailable",
            "executor store is not mounted",
        );
    };
    if let Err(message) = validate_status(&request.status, request.expected_revision) {
        return error(
            StatusCode::BAD_REQUEST,
            "fleet.invalid_observation",
            &message,
        );
    }
    let current = match store.get_fleet_workload(&child_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "fleet.workload_not_found",
                "workload was not found",
            )
        }
        Err(cause) => return internal("fleet.observation_failed", cause),
    };
    if current.workload_kind == "daemon"
        && !matches!(
            request.status["health"].as_str(),
            Some("healthy" | "degraded" | "unhealthy" | "unknown")
        )
    {
        return error(
            StatusCode::BAD_REQUEST,
            "fleet.invalid_observation",
            "daemon observations require a valid status.health",
        );
    }
    let revision = request.status["revision"].as_u64().unwrap();
    let observed_state = request.status["observed_state"]
        .as_str()
        .unwrap()
        .to_string();
    let last_seen = match request.status["last_seen"].as_str().unwrap().parse() {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "fleet.invalid_observation",
                "status.last_seen must be an RFC3339 timestamp",
            )
        }
    };
    let mut next = current.clone();
    if let Err(cause) =
        apply_runtime_identity(&mut next.record_json, request.runtime_identity.as_ref())
    {
        return match cause {
            RuntimeIdentityError::Invalid(message) => error(
                StatusCode::BAD_REQUEST,
                "fleet.invalid_runtime_identity",
                message,
            ),
            RuntimeIdentityError::Immutable(message) => error(
                StatusCode::CONFLICT,
                "fleet.runtime_identity_immutable",
                message,
            ),
        };
    }
    next.revision = revision;
    next.observed_state = observed_state;
    next.last_seen = last_seen;
    next.updated_at = Utc::now();
    next.record_json["status"] = request.status;

    match store.observe_fleet_workload(&child_id, request.expected_revision, &next) {
        Ok(FleetObservationOutcome::Updated(row)) => {
            (StatusCode::OK, Json(row.record_json)).into_response()
        }
        Ok(FleetObservationOutcome::Missing) => error(
            StatusCode::NOT_FOUND,
            "fleet.workload_not_found",
            "workload was not found",
        ),
        Ok(FleetObservationOutcome::Stale { current_revision }) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "fleet.stale_revision",
                "message": "observation did not advance the current revision",
                "current_revision": current_revision,
            })),
        )
            .into_response(),
        Err(cause) => internal("fleet.observation_failed", cause),
    }
}

enum RuntimeIdentityError {
    Invalid(&'static str),
    Immutable(&'static str),
}

fn apply_runtime_identity(
    record: &mut Value,
    identity: Option<&RuntimeIdentityRequest>,
) -> Result<(), RuntimeIdentityError> {
    let Some(identity) = identity else {
        return Ok(());
    };
    if identity.session_id.is_none() && identity.task_id.is_none() && identity.command_id.is_none()
    {
        return Err(RuntimeIdentityError::Invalid(
            "runtime_identity must assign at least one identity",
        ));
    }
    for (field, candidate) in [
        ("session_id", identity.session_id.as_deref()),
        ("task_id", identity.task_id.as_deref()),
        ("command_id", identity.command_id.as_deref()),
    ] {
        let Some(candidate) = candidate else {
            continue;
        };
        if candidate.is_empty() {
            return Err(RuntimeIdentityError::Invalid(
                "runtime identities must be non-empty strings",
            ));
        }
        let current = record["lineage"][field].as_str();
        if current.is_some_and(|current| current != candidate) {
            return Err(RuntimeIdentityError::Immutable(
                "assigned runtime identities cannot be changed",
            ));
        }
        record["lineage"][field] = json!(candidate);
    }
    Ok(())
}

async fn reconcile(
    State(state): State<AppState>,
    _admin: RequireAdmin,
    Json(request): Json<ReconcileRequest>,
) -> Response {
    if request.child_ids.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "fleet.empty_reconciliation",
            "child_ids must contain at least one workload",
        );
    }
    let Some(store) = state.executor_task_store.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fleet.store_unavailable",
            "executor store is not mounted",
        );
    };
    let inventory = match store.list_fleet_workloads() {
        Ok(rows) => rows,
        Err(cause) => return internal("fleet.reconciliation_failed", cause),
    };
    let after_revision = inventory_revision(&inventory);
    if reconciliation_revision_is_stale(request.before_revision, after_revision) {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "fleet.stale_inventory_revision",
                "message": "fleet inventory changed after the reviewed reconciliation plan",
                "expected_revision": request.before_revision,
                "current_revision": after_revision,
            })),
        )
            .into_response();
    }
    let rows = request
        .child_ids
        .iter()
        .map(|child_id| {
            let Some(row) = inventory.iter().find(|row| &row.child_id == child_id) else {
                return json!({
                    "child_id": child_id,
                    "classification": "unknown",
                    "observed_state": "unknown",
                    "revision": 0,
                    "reason": "expected workload is absent from durable inventory",
                });
            };
            let classification = match row.observed_state.as_str() {
                "succeeded" => "terminal",
                "failed" | "cancelled" | "timed-out" => "failed-or-aborted",
                "unknown" | "operator-review-required" => "operator-review-required",
                _ => "re-adopted",
            };
            json!({
                "child_id": child_id,
                "classification": classification,
                "observed_state": row.observed_state,
                "revision": row.revision,
                "reason": "classification derived from durable workload inventory",
            })
        })
        .collect::<Vec<_>>();
    let result = json!({
        "document_type": "reconciliation",
        "api_version": API_VERSION,
        "generated_at": Utc::now().to_rfc3339(),
        "before_revision": request.before_revision,
        "after_revision": after_revision,
        "rows": rows,
    });
    let Some(operation_store) = state.operation_store.as_ref() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "fleet.operation_store_unavailable",
            "management operation store is not mounted",
        );
    };
    let mut operation = Operation::new(OperationType::FleetReconcile, "fleet".to_string());
    operation.state = OperationState::Completed;
    operation.completed_at = Some(Utc::now());
    operation.progress_percent = 100;
    operation.result = Some(result.clone());
    let operation_id = operation_store.insert(operation);
    let mut response = (StatusCode::OK, Json(result)).into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&format!("/api/v2/admin/operations/{operation_id}"))
            .unwrap_or_else(|_| HeaderValue::from_static("/api/v2/admin/operations")),
    );
    response.headers_mut().insert(
        "operation-id",
        HeaderValue::from_str(&operation_id)
            .unwrap_or_else(|_| HeaderValue::from_static("unknown")),
    );
    response
}

fn reconciliation_revision_is_stale(reviewed: u64, current: u64) -> bool {
    reviewed != current
}

fn validate_dispatch_record(record: &Value) -> Result<(), String> {
    reject_unknown_keys(
        record,
        "",
        &[
            "document_type",
            "api_version",
            "kind",
            "lineage",
            "spec",
            "status",
        ],
    )?;
    reject_unknown_keys(
        &record["lineage"],
        "/lineage",
        &[
            "orchestrator_id",
            "mission_id",
            "dispatch_id",
            "idempotency_key",
            "parent_id",
            "child_id",
            "target_id",
            "executor_id",
            "runtime_id",
            "session_id",
            "task_id",
            "command_id",
        ],
    )?;
    reject_unknown_keys(
        &record["spec"],
        "/spec",
        &[
            "desired_state",
            "capabilities",
            "policy",
            "budgets",
            "schedule",
            "orchestrator_metadata",
        ],
    )?;
    reject_unknown_keys(
        &record["status"],
        "/status",
        &[
            "observed_state",
            "revision",
            "last_seen",
            "health",
            "backpressure",
            "artifacts",
            "exit_classification",
            "error_code",
        ],
    )?;
    if record["document_type"] != "workload" || record["api_version"] != API_VERSION {
        return Err("record must be a workload using agentic-orchestration/v1".into());
    }
    let kind = required_string(record, "/kind")?;
    if !matches!(
        kind.as_str(),
        "persistent-agent" | "daemon" | "scheduled-collector" | "one-shot-command"
    ) {
        return Err("kind is not a supported fleet workload kind".into());
    }
    for field in [
        "/lineage/orchestrator_id",
        "/lineage/mission_id",
        "/lineage/dispatch_id",
        "/lineage/idempotency_key",
        "/lineage/child_id",
        "/lineage/target_id",
        "/lineage/executor_id",
        "/lineage/runtime_id",
    ] {
        required_string(record, field)?;
    }
    for field in [
        "/lineage/session_id",
        "/lineage/task_id",
        "/lineage/command_id",
    ] {
        optional_nullable_string(record, field)?;
    }
    if record.pointer("/status/revision").and_then(Value::as_u64) != Some(0)
        || record
            .pointer("/status/observed_state")
            .and_then(Value::as_str)
            != Some("pending")
    {
        return Err("new workloads must start at pending revision 0".into());
    }
    required_string(record, "/status/last_seen")?;
    if !record["spec"]["capabilities"].is_array()
        || !record["spec"]["policy"].is_object()
        || !record["spec"]["budgets"].is_object()
        || !record["status"]["artifacts"].is_array()
    {
        return Err("spec capabilities/policy/budgets and status.artifacts are required".into());
    }
    required_string(record, "/spec/desired_state")?;
    if kind == "scheduled-collector" {
        required_string(record, "/spec/schedule")?;
    }
    if kind == "daemon"
        && record
            .pointer("/status/health")
            .and_then(Value::as_str)
            .is_none()
    {
        return Err("daemon workloads require status.health".into());
    }
    reject_secret_material(record, "")
}

fn validate_status(status: &Value, expected_revision: u64) -> Result<(), String> {
    reject_unknown_keys(
        status,
        "/status",
        &[
            "observed_state",
            "revision",
            "last_seen",
            "health",
            "backpressure",
            "artifacts",
            "exit_classification",
            "error_code",
        ],
    )?;
    let revision = status["revision"]
        .as_u64()
        .ok_or("status.revision must be an integer")?;
    if revision != expected_revision + 1 {
        return Err("status.revision must advance expected_revision by exactly one".into());
    }
    let observed = status["observed_state"]
        .as_str()
        .ok_or("status.observed_state is required")?;
    if !matches!(
        observed,
        "pending"
            | "admitted"
            | "starting"
            | "running"
            | "blocked"
            | "detached"
            | "retained"
            | "healthy"
            | "degraded"
            | "restarting"
            | "scheduled"
            | "missed"
            | "catching-up"
            | "stopping"
            | "succeeded"
            | "failed"
            | "cancelled"
            | "timed-out"
            | "unknown"
            | "operator-review-required"
    ) {
        return Err("status.observed_state is invalid".into());
    }
    required_string(status, "/last_seen")?;
    if !status["artifacts"].is_array() {
        return Err("status.artifacts must be an array".into());
    }
    reject_secret_material(status, "/status")
}

fn reject_unknown_keys(value: &Value, path: &str, allowed: &[&str]) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{path} must be an object"))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown contract field at {path}/{key}"));
    }
    Ok(())
}

fn required_string(value: &Value, pointer: &str) -> Result<String, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{pointer} must be a non-empty string"))
}

fn optional_nullable_string(value: &Value, pointer: &str) -> Result<(), String> {
    match value.pointer(pointer) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(identity)) if !identity.is_empty() => Ok(()),
        _ => Err(format!("{pointer} must be null or a non-empty string")),
    }
}

fn reject_secret_material(value: &Value, path: &str) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let lower = key.to_ascii_lowercase();
                let permitted_reference = matches!(
                    lower.as_str(),
                    "credential_policy_ref" | "network_policy_ref"
                );
                if !permitted_reference
                    && matches!(
                        lower.as_str(),
                        "credential"
                            | "credentials"
                            | "secret"
                            | "secrets"
                            | "password"
                            | "token"
                            | "private_key"
                            | "access_key"
                            | "api_key"
                    )
                {
                    return Err(format!("credential material is forbidden at {path}/{key}"));
                }
                reject_secret_material(child, &format!("{path}/{key}"))?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                reject_secret_material(child, &format!("{path}/{index}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn canonical_hash(value: &Value) -> Result<String, String> {
    let canonical = serde_jcs::to_string(value).map_err(|cause| cause.to_string())?;
    Ok(hex::encode(Sha256::digest(canonical.as_bytes())))
}

fn dispatch_request_hash(record: &Value) -> Result<String, String> {
    let mut intent = record.clone();
    if let Some(status) = intent.get_mut("status").and_then(Value::as_object_mut) {
        // last_seen is required for wire/schema fidelity but changes when the
        // same durable dispatch is retried after a crash. It is server-owned
        // observation metadata, not part of immutable admission intent.
        status.remove("last_seen");
    }
    canonical_hash(&intent)
}

fn inventory_revision(rows: &[FleetWorkloadRow]) -> u64 {
    rows.iter().fold(rows.len() as u64, |sum, row| {
        sum.saturating_add(row.revision)
    })
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"error": code, "message": message}))).into_response()
}

fn internal(code: &str, cause: anyhow::Error) -> Response {
    tracing::error!(error = %cause, code, "fleet workload operation failed");
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        code,
        "fleet workload operation failed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Value {
        json!({
            "document_type": "workload",
            "api_version": API_VERSION,
            "kind": "one-shot-command",
            "lineage": {
                "orchestrator_id": "orchestrator", "mission_id": "mission", "dispatch_id": "dispatch",
                "idempotency_key": "key", "child_id": "child", "target_id": "target",
                "executor_id": "executor", "runtime_id": "runtime",
                "session_id": null, "task_id": null, "command_id": null
            },
            "spec": {"desired_state": "running", "capabilities": [], "policy": {
                "trust_tier": "T2", "isolation_kind": "vm", "credential_policy_ref": "policy-ref"
            }, "budgets": {"max_attempts": 1, "timeout_seconds": 60}},
            "status": {"observed_state": "pending", "revision": 0,
                "last_seen": "2026-08-02T12:00:00Z", "artifacts": []}
        })
    }

    fn fleet_row(kind: &str, observed_state: &str, revision: u64) -> FleetWorkloadRow {
        let now = Utc::now();
        let mut record = record();
        record["kind"] = json!(kind);
        record["lineage"]["task_id"] = json!("task-1");
        record["status"]["observed_state"] = json!(observed_state);
        record["status"]["revision"] = json!(revision);
        if kind == "daemon" {
            record["status"]["health"] = json!("unknown");
        }
        FleetWorkloadRow {
            child_id: "child".into(),
            idempotency_key: "key".into(),
            request_hash: "hash".into(),
            target_id: "target".into(),
            executor_id: "executor".into(),
            workload_kind: kind.into(),
            observed_state: observed_state.into(),
            revision,
            last_seen: now,
            record_json: record,
            created_at: now,
            updated_at: now,
        }
    }

    fn task(state: TaskState) -> TaskRow {
        let now = Utc::now();
        TaskRow {
            task_id: "task-1".into(),
            context_id: None,
            instance_id: Some("target".into()),
            state,
            fail_kind: None,
            status_json: json!({"state": state.as_str(), "exit_code": 0}),
            metadata_json: None,
            created_at: now,
            updated_at: now,
            terminal_at: state.is_terminal().then_some(now),
        }
    }

    #[test]
    fn validates_neutral_record_and_rejects_secret_material() {
        assert!(validate_dispatch_record(&record()).is_ok());
        let mut unsafe_record = record();
        unsafe_record["spec"]["orchestrator_metadata"] = json!({"token": "not-allowed"});
        assert!(validate_dispatch_record(&unsafe_record)
            .unwrap_err()
            .contains("forbidden"));
        let mut invalid_identity = record();
        invalid_identity["lineage"]["task_id"] = json!({"opaque": "not-an-id"});
        assert!(validate_dispatch_record(&invalid_identity)
            .unwrap_err()
            .contains("null or a non-empty string"));
    }

    #[test]
    fn observations_must_advance_exactly_once() {
        let status = json!({"observed_state": "running", "revision": 2,
            "last_seen": "2026-08-02T12:00:01Z", "artifacts": []});
        assert!(validate_status(&status, 1).is_ok());
        assert!(validate_status(&status, 0).is_err());
    }

    #[test]
    fn reconciliation_refuses_a_stale_reviewed_inventory_revision() {
        assert!(!reconciliation_revision_is_stale(7, 7));
        assert!(reconciliation_revision_is_stale(7, 8));
    }

    #[test]
    fn runtime_identities_are_assigned_once_and_remain_immutable() {
        let mut workload = record();
        let first = RuntimeIdentityRequest {
            session_id: Some("session-1".into()),
            task_id: Some("task-1".into()),
            command_id: None,
        };
        assert!(apply_runtime_identity(&mut workload, Some(&first)).is_ok());
        assert_eq!(workload["lineage"]["session_id"], "session-1");
        assert_eq!(workload["lineage"]["task_id"], "task-1");

        // Re-reporting the same binding on a later monotonic observation is safe.
        assert!(apply_runtime_identity(&mut workload, Some(&first)).is_ok());
        let reassignment = RuntimeIdentityRequest {
            session_id: Some("session-2".into()),
            task_id: None,
            command_id: None,
        };
        assert!(matches!(
            apply_runtime_identity(&mut workload, Some(&reassignment)),
            Err(RuntimeIdentityError::Immutable(_))
        ));
        assert_eq!(workload["lineage"]["session_id"], "session-1");
    }

    #[test]
    fn runtime_identity_updates_must_bind_a_nonempty_identity() {
        let mut workload = record();
        let empty = RuntimeIdentityRequest {
            session_id: None,
            task_id: None,
            command_id: None,
        };
        assert!(matches!(
            apply_runtime_identity(&mut workload, Some(&empty)),
            Err(RuntimeIdentityError::Invalid(_))
        ));
        let blank = RuntimeIdentityRequest {
            session_id: None,
            task_id: Some(String::new()),
            command_id: None,
        };
        assert!(matches!(
            apply_runtime_identity(&mut workload, Some(&blank)),
            Err(RuntimeIdentityError::Invalid(_))
        ));
    }

    #[test]
    fn idempotency_hash_ignores_only_the_volatile_admission_timestamp() {
        let first = record();
        let mut retry = first.clone();
        retry["status"]["last_seen"] = json!("2026-08-02T12:05:00Z");
        assert_eq!(
            dispatch_request_hash(&first).unwrap(),
            dispatch_request_hash(&retry).unwrap()
        );

        retry["lineage"]["target_id"] = json!("different-target");
        assert_ne!(
            dispatch_request_hash(&first).unwrap(),
            dispatch_request_hash(&retry).unwrap()
        );
    }

    #[test]
    fn bound_one_shot_tasks_project_runtime_state_evidence_and_command_identity() {
        let current = fleet_row("one-shot-command", "starting", 2);
        let artifact = ArtifactRow {
            artifact_id: "stdout-1".into(),
            task_id: "task-1".into(),
            artifact_json: json!({"stream": "stdout", "data": "ok", "command_id": "command-1"}),
            created_at: Utc::now(),
        };
        let projected =
            project_task_observation(&current, &task(TaskState::Completed), &[artifact])
                .expect("terminal task must advance fleet observation");
        assert_eq!(projected.observed_state, "succeeded");
        assert_eq!(projected.revision, 3);
        assert_eq!(projected.record_json["lineage"]["command_id"], "command-1");
        assert_eq!(
            projected.record_json["status"]["exit_classification"],
            "success"
        );
        assert_eq!(
            projected.record_json["status"]["artifacts"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn daemon_task_exit_requires_review_instead_of_claiming_health() {
        let current = fleet_row("daemon", "healthy", 4);
        let projected = project_task_observation(&current, &task(TaskState::Completed), &[])
            .expect("daemon exit must change posture");
        assert_eq!(projected.observed_state, "operator-review-required");
        assert_eq!(projected.record_json["status"]["health"], "unknown");
        assert_eq!(
            projected.record_json["status"]["error_code"],
            "fleet.daemon_task_exited"
        );
    }

    #[test]
    fn unchanged_task_projection_does_not_churn_inventory_revision() {
        let mut current = fleet_row("persistent-agent", "running", 3);
        current.record_json["status"]["artifacts"] = json!([]);
        assert!(project_task_observation(&current, &task(TaskState::Working), &[]).is_none());
    }
}

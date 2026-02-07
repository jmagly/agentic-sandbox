//! Operation tracking for long-running VM operations
//!
//! Tracks asynchronous VM operations (create, restart, etc.) with progress
//! and result information. Operations are stored in memory with TTL.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::warn;
use uuid::Uuid;

use super::server::AppState;
use super::vms::VmInfo;

/// Operation tracking store with TTL
pub struct OperationStore {
    operations: Arc<DashMap<String, Operation>>,
}

impl OperationStore {
    pub fn new() -> Self {
        let store = Self {
            operations: Arc::new(DashMap::new()),
        };

        // Spawn background cleanup task
        let ops = store.operations.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
            loop {
                interval.tick().await;
                Self::cleanup_expired(&ops);
            }
        });

        store
    }

    /// Insert a new operation
    pub fn insert(&self, operation: Operation) -> String {
        let id = operation.id.clone();
        self.operations.insert(id.clone(), operation);
        id
    }

    /// Get an operation by ID
    pub fn get(&self, id: &str) -> Option<Operation> {
        self.operations.get(id).map(|op| op.clone())
    }

    /// Update operation state
    pub fn update_state(&self, id: &str, state: OperationState) {
        if let Some(mut op) = self.operations.get_mut(id) {
            if matches!(state, OperationState::Completed | OperationState::Failed { .. }) {
                op.completed_at = Some(Utc::now());
            }
            op.state = state;
        }
    }

    /// Update operation progress
    pub fn update_progress(&self, id: &str, progress: u8) {
        if let Some(mut op) = self.operations.get_mut(id) {
            op.progress_percent = progress;
        }
    }

    /// Mark operation as failed
    pub fn mark_failed(&self, id: &str, error: String) {
        if let Some(mut op) = self.operations.get_mut(id) {
            op.state = OperationState::Failed { error: error.clone() };
            op.completed_at = Some(Utc::now());
        }
    }

    /// Mark operation as completed with result
    pub fn mark_completed(&self, id: &str, result: Option<serde_json::Value>) {
        if let Some(mut op) = self.operations.get_mut(id) {
            op.state = OperationState::Completed;
            op.completed_at = Some(Utc::now());
            op.progress_percent = 100;
            op.result = result;
        }
    }

    /// Remove expired operations (older than 1 hour)
    fn cleanup_expired(operations: &DashMap<String, Operation>) {
        let cutoff = Utc::now() - chrono::Duration::hours(1);
        let expired: Vec<String> = operations
            .iter()
            .filter(|entry| {
                entry.value().completed_at.map_or(false, |completed| completed < cutoff)
            })
            .map(|entry| entry.key().clone())
            .collect();

        for id in expired {
            operations.remove(&id);
            warn!(operation_id = %id, "Cleaned up expired operation");
        }
    }
}

impl Default for OperationStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Long-running operation tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    #[serde(rename = "type")]
    pub op_type: OperationType,
    pub state: OperationState,
    pub target: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub progress_percent: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

impl Operation {
    /// Create a new operation
    pub fn new(op_type: OperationType, target: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            op_type,
            state: OperationState::Pending,
            target,
            created_at: Utc::now(),
            completed_at: None,
            progress_percent: 0,
            result: None,
        }
    }

    /// Create operation response
    pub fn to_response(&self) -> OperationResponse {
        OperationResponse {
            id: self.id.clone(),
            op_type: self.op_type.clone(),
            state: self.state.clone(),
            target: self.target.clone(),
            created_at: self.created_at,
            completed_at: self.completed_at,
            progress_percent: self.progress_percent,
            result: self.result.clone(),
        }
    }
}

/// Operation type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationType {
    VmCreate,
    VmDelete,
    VmRestart,
}

/// Operation state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase", tag = "status")]
pub enum OperationState {
    Pending,
    Running,
    Completed,
    #[serde(rename = "failed")]
    Failed { error: String },
}

/// Response for operation status
#[derive(Debug, Serialize)]
pub struct OperationResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub op_type: OperationType,
    #[serde(flatten)]
    pub state: OperationState,
    pub target: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub progress_percent: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// Response for create operation
#[derive(Debug, Serialize)]
pub struct CreateOperationResponse {
    pub operation: OperationResponse,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm: Option<VmInfo>,
}

/// Error types for operations
#[derive(Debug, Error)]
pub enum OperationError {
    #[error("Operation not found: {0}")]
    NotFound(String),
}

impl OperationError {
    fn status_code(&self) -> StatusCode {
        match self {
            OperationError::NotFound(_) => StatusCode::NOT_FOUND,
        }
    }

    fn error_code(&self) -> &'static str {
        match self {
            OperationError::NotFound(_) => "OPERATION_NOT_FOUND",
        }
    }
}

impl IntoResponse for OperationError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status_code();
        let code = self.error_code();
        let message = self.to_string();

        let body = Json(ErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message,
            },
        });

        (status, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: String,
    message: String,
}

/// GET /api/v1/operations/{id} - Get operation status
pub async fn get_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<OperationResponse>, OperationError> {
    let store = state
        .operation_store
        .as_ref()
        .ok_or_else(|| OperationError::NotFound(id.clone()))?;

    let operation = store
        .get(&id)
        .ok_or_else(|| OperationError::NotFound(id.clone()))?;

    Ok(Json(operation.to_response()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_new() {
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());

        assert_eq!(op.op_type, OperationType::VmCreate);
        assert_eq!(op.target, "agent-01");
        assert_eq!(op.state, OperationState::Pending);
        assert_eq!(op.progress_percent, 0);
        assert!(op.completed_at.is_none());
        assert!(op.result.is_none());
        assert!(!op.id.is_empty());
    }

    #[test]
    fn test_operation_type_serialization() {
        let op_type = OperationType::VmCreate;
        let json = serde_json::to_string(&op_type).unwrap();
        assert_eq!(json, r#""vm_create""#);

        let op_type = OperationType::VmDelete;
        let json = serde_json::to_string(&op_type).unwrap();
        assert_eq!(json, r#""vm_delete""#);

        let op_type = OperationType::VmRestart;
        let json = serde_json::to_string(&op_type).unwrap();
        assert_eq!(json, r#""vm_restart""#);
    }

    #[test]
    fn test_operation_state_serialization() {
        let state = OperationState::Pending;
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "pending");

        let state = OperationState::Running;
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "running");

        let state = OperationState::Completed;
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "completed");

        let state = OperationState::Failed {
            error: "test error".to_string(),
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["status"], "failed");
        assert_eq!(json["error"], "test error");
    }

    #[tokio::test]
    async fn test_operation_store_insert_get() {
        let store = OperationStore::new();
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());
        let id = op.id.clone();

        let stored_id = store.insert(op);
        assert_eq!(stored_id, id);

        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.op_type, OperationType::VmCreate);
        assert_eq!(retrieved.target, "agent-01");
    }

    #[tokio::test]
    async fn test_operation_store_update_state() {
        let store = OperationStore::new();
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());
        let id = store.insert(op);

        store.update_state(&id, OperationState::Running);
        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.state, OperationState::Running);
        assert!(retrieved.completed_at.is_none());

        store.update_state(&id, OperationState::Completed);
        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.state, OperationState::Completed);
        assert!(retrieved.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_operation_store_update_progress() {
        let store = OperationStore::new();
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());
        let id = store.insert(op);

        store.update_progress(&id, 50);
        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.progress_percent, 50);

        store.update_progress(&id, 100);
        let retrieved = store.get(&id).unwrap();
        assert_eq!(retrieved.progress_percent, 100);
    }

    #[tokio::test]
    async fn test_operation_store_mark_failed() {
        let store = OperationStore::new();
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());
        let id = store.insert(op);

        store.mark_failed(&id, "Provisioning failed".to_string());
        let retrieved = store.get(&id).unwrap();

        match retrieved.state {
            OperationState::Failed { error } => {
                assert_eq!(error, "Provisioning failed");
            }
            _ => panic!("Expected Failed state"),
        }
        assert!(retrieved.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_operation_store_mark_completed() {
        let store = OperationStore::new();
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());
        let id = store.insert(op);

        let result = serde_json::json!({
            "vm": {
                "name": "agent-01",
                "state": "running"
            }
        });

        store.mark_completed(&id, Some(result.clone()));
        let retrieved = store.get(&id).unwrap();

        assert_eq!(retrieved.state, OperationState::Completed);
        assert_eq!(retrieved.progress_percent, 100);
        assert!(retrieved.completed_at.is_some());
        assert_eq!(retrieved.result, Some(result));
    }

    #[test]
    fn test_operation_to_response() {
        let op = Operation::new(OperationType::VmCreate, "agent-01".to_string());
        let response = op.to_response();

        assert_eq!(response.id, op.id);
        assert_eq!(response.op_type, OperationType::VmCreate);
        assert_eq!(response.target, "agent-01");
        assert_eq!(response.progress_percent, 0);
    }

    #[test]
    fn test_operation_error_codes() {
        let err = OperationError::NotFound("op-123".to_string());
        assert_eq!(err.error_code(), "OPERATION_NOT_FOUND");
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);
    }
}

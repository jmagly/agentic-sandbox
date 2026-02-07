//! HTTP endpoints for task management
//!
//! REST API for submitting, listing, and managing orchestrated tasks.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;
#[allow(unused_imports)] // Used by Sse stream
use tokio_stream::StreamExt;
use tracing::{error, info};

use crate::orchestrator::{
    manifest::TaskManifest,
    monitor::TaskOutputEvent,
    task::{Task, TaskState},
};

use super::server::AppState;

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct SubmitTaskRequest {
    /// YAML manifest as string
    #[serde(default)]
    pub manifest_yaml: Option<String>,
    /// JSON manifest (alternative to YAML)
    #[serde(default)]
    pub manifest: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct SubmitTaskResponse {
    pub task_id: String,
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskStatusResponse {
    pub id: String,
    pub name: String,
    pub state: String,
    pub state_message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub state_changed_at: String,
    pub vm_name: Option<String>,
    pub vm_ip: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub progress: TaskProgressResponse,
}

#[derive(Debug, Serialize)]
pub struct TaskProgressResponse {
    pub output_bytes: u64,
    pub tool_calls: u32,
    pub current_tool: Option<String>,
    pub last_activity_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskListResponse {
    pub tasks: Vec<TaskStatusResponse>,
    pub total_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct TaskListQuery {
    /// Filter by state (comma-separated)
    #[serde(default)]
    pub state: Option<String>,
    /// Max results
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination
    #[serde(default)]
    pub offset: usize,
}

fn default_limit() -> usize { 50 }

#[derive(Debug, Deserialize)]
pub struct CancelTaskRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CancelTaskResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ArtifactResponse {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub content_type: String,
    pub checksum: String,
}

#[derive(Debug, Serialize)]
pub struct ArtifactListResponse {
    pub artifacts: Vec<ArtifactResponse>,
}

// =============================================================================
// Handlers
// =============================================================================

/// POST /api/v1/tasks - Submit a new task
pub async fn submit_task(
    State(state): State<AppState>,
    Json(request): Json<SubmitTaskRequest>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(SubmitTaskResponse {
                    task_id: String::new(),
                    accepted: false,
                    error: Some("Orchestrator not initialized".to_string()),
                }),
            );
        }
    };

    // Parse manifest from YAML or JSON
    let manifest = if let Some(yaml) = request.manifest_yaml {
        match TaskManifest::from_yaml(&yaml) {
            Ok(m) => m.with_generated_id(),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SubmitTaskResponse {
                        task_id: String::new(),
                        accepted: false,
                        error: Some(format!("Invalid YAML manifest: {}", e)),
                    }),
                );
            }
        }
    } else if let Some(json_val) = request.manifest {
        match serde_json::from_value::<TaskManifest>(json_val) {
            Ok(m) => m.with_generated_id(),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SubmitTaskResponse {
                        task_id: String::new(),
                        accepted: false,
                        error: Some(format!("Invalid JSON manifest: {}", e)),
                    }),
                );
            }
        }
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(SubmitTaskResponse {
                task_id: String::new(),
                accepted: false,
                error: Some("Either manifest_yaml or manifest required".to_string()),
            }),
        );
    };

    // Submit to orchestrator
    match orchestrator.submit_task(manifest).await {
        Ok(task_id) => {
            info!("Task {} submitted successfully", task_id);
            (
                StatusCode::ACCEPTED,
                Json(SubmitTaskResponse {
                    task_id,
                    accepted: true,
                    error: None,
                }),
            )
        }
        Err(e) => {
            error!("Failed to submit task: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(SubmitTaskResponse {
                    task_id: String::new(),
                    accepted: false,
                    error: Some(e.to_string()),
                }),
            )
        }
    }
}

/// GET /api/v1/tasks - List tasks
pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(TaskListResponse {
                    tasks: vec![],
                    total_count: 0,
                }),
            );
        }
    };

    // Parse state filter
    let state_filter = query.state.map(|s| {
        s.split(',')
            .filter_map(|state_str| match state_str.trim() {
                "pending" => Some(TaskState::Pending),
                "staging" => Some(TaskState::Staging),
                "provisioning" => Some(TaskState::Provisioning),
                "ready" => Some(TaskState::Ready),
                "running" => Some(TaskState::Running),
                "completing" => Some(TaskState::Completing),
                "completed" => Some(TaskState::Completed),
                "failed" => Some(TaskState::Failed),
                "failed_preserved" => Some(TaskState::FailedPreserved),
                "cancelled" => Some(TaskState::Cancelled),
                _ => None,
            })
            .collect()
    });

    let tasks = orchestrator.list_tasks(state_filter).await;
    let total_count = tasks.len();

    // Apply pagination
    let tasks: Vec<TaskStatusResponse> = tasks
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .map(task_to_response)
        .collect();

    (
        StatusCode::OK,
        Json(TaskListResponse { tasks, total_count }),
    )
}

/// GET /api/v1/tasks/:id - Get task status
pub async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Orchestrator not initialized"})),
            ).into_response();
        }
    };

    match orchestrator.get_task(&task_id).await {
        Some(task) => {
            (StatusCode::OK, Json(task_to_response(task))).into_response()
        }
        None => {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Task not found"})),
            ).into_response()
        }
    }
}

/// DELETE /api/v1/tasks/:id - Cancel a task
pub async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<CancelTaskRequest>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(CancelTaskResponse {
                    success: false,
                    error: Some("Orchestrator not initialized".to_string()),
                }),
            );
        }
    };

    let reason = request.reason.unwrap_or_else(|| "User requested cancellation".to_string());

    match orchestrator.cancel_task(&task_id, &reason).await {
        Ok(_) => {
            info!("Task {} cancelled: {}", task_id, reason);
            (
                StatusCode::OK,
                Json(CancelTaskResponse {
                    success: true,
                    error: None,
                }),
            )
        }
        Err(e) => {
            error!("Failed to cancel task {}: {}", task_id, e);
            (
                StatusCode::BAD_REQUEST,
                Json(CancelTaskResponse {
                    success: false,
                    error: Some(e.to_string()),
                }),
            )
        }
    }
}

/// GET /api/v1/tasks/:id/logs - Stream task logs via SSE
pub async fn stream_task_logs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Orchestrator not initialized",
            ));
        }
    };

    // Verify task exists
    if orchestrator.get_task(&task_id).await.is_none() {
        return Err((StatusCode::NOT_FOUND, "Task not found"));
    }

    // Subscribe to monitor events
    let monitor = orchestrator.monitor();
    let mut rx = monitor.subscribe();

    // Start monitoring if not already
    monitor.start_monitoring(&task_id).await;

    // Create SSE stream
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    match &event {
                        TaskOutputEvent::Stdout(id, data) if id == &task_id => {
                            let text = String::from_utf8_lossy(data);
                            yield Ok::<_, Infallible>(axum::response::sse::Event::default()
                                .event("stdout")
                                .data(&text));
                        }
                        TaskOutputEvent::Stderr(id, data) if id == &task_id => {
                            let text = String::from_utf8_lossy(data);
                            yield Ok::<_, Infallible>(axum::response::sse::Event::default()
                                .event("stderr")
                                .data(&text));
                        }
                        TaskOutputEvent::Event(id, event) if id == &task_id => {
                            yield Ok::<_, Infallible>(axum::response::sse::Event::default()
                                .event("event")
                                .data(serde_json::to_string(&event).unwrap_or_default()));
                        }
                        TaskOutputEvent::Completed(id, code) if id == &task_id => {
                            yield Ok::<_, Infallible>(axum::response::sse::Event::default()
                                .event("completed")
                                .data(code.to_string()));
                            break;
                        }
                        TaskOutputEvent::Error(id, err) if id == &task_id => {
                            yield Ok::<_, Infallible>(axum::response::sse::Event::default()
                                .event("error")
                                .data(err.clone()));
                            break;
                        }
                        _ => {}
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Skip lagged messages
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

/// GET /api/v1/tasks/:id/artifacts - List task artifacts
pub async fn list_artifacts(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ArtifactListResponse { artifacts: vec![] }),
            ).into_response();
        }
    };

    // Verify task exists
    if orchestrator.get_task(&task_id).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Task not found"})),
        ).into_response();
    }

    let storage = orchestrator.storage();
    match storage.list_artifacts(&task_id).await {
        Ok(artifacts) => {
            let artifacts = artifacts
                .into_iter()
                .map(|a| ArtifactResponse {
                    name: a.name,
                    path: a.path,
                    size_bytes: a.size_bytes,
                    content_type: a.content_type,
                    checksum: String::new(), // TODO: compute checksum
                })
                .collect();

            (StatusCode::OK, Json(ArtifactListResponse { artifacts })).into_response()
        }
        Err(e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response()
        }
    }
}

/// GET /api/v1/tasks/:id/artifacts/:name - Download artifact
pub async fn download_artifact(
    State(state): State<AppState>,
    Path((task_id, artifact_name)): Path<(String, String)>,
) -> impl IntoResponse {
    let orchestrator = match &state.orchestrator {
        Some(o) => o,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Orchestrator not initialized",
            ).into_response();
        }
    };

    let storage = orchestrator.storage();
    let artifact_path = storage.artifacts_path(&task_id).join(&artifact_name);

    if !artifact_path.exists() {
        return (StatusCode::NOT_FOUND, "Artifact not found").into_response();
    }

    match tokio::fs::read(&artifact_path).await {
        Ok(content) => {
            let content_type = mime_guess::from_path(&artifact_path)
                .first_or_octet_stream()
                .to_string();

            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", artifact_name),
                )
                .body(Body::from(content))
                .unwrap()
                .into_response()
        }
        Err(e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read artifact: {}", e),
            ).into_response()
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn task_to_response(task: Task) -> TaskStatusResponse {
    TaskStatusResponse {
        id: task.id,
        name: task.name,
        state: task.state.to_string(),
        state_message: task.state_message,
        created_at: task.created_at.to_rfc3339(),
        started_at: task.started_at.map(|t| t.to_rfc3339()),
        state_changed_at: task.state_changed_at.to_rfc3339(),
        vm_name: task.vm_name,
        vm_ip: task.vm_ip,
        exit_code: task.exit_code,
        error: task.error,
        progress: TaskProgressResponse {
            output_bytes: task.progress.output_bytes,
            tool_calls: task.progress.tool_calls,
            current_tool: task.progress.current_tool,
            last_activity_at: task.progress.last_activity_at.map(|t| t.to_rfc3339()),
        },
    }
}

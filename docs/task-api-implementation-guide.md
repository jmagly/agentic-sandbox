# Task Orchestration API - Implementation Guide

Version: 1.0.0
Date: 2026-01-29
For: Software Implementers, Backend Engineers

## Overview

This document provides implementation guidance for the Task Orchestration API. It covers the technical architecture, implementation phases, data models, and specific code patterns for the Rust backend.

---

## 1. Architecture Overview

### 1.1 Component Interaction

```
┌─────────────────────────────────────────────────────────────────┐
│                         HTTP Layer (Axum)                        │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌───────────┐│
│  │ POST /tasks│  │ GET /tasks │  │GET /logs   │  │GET /artifacts││
│  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘  └─────┬─────┘│
│        │               │               │               │       │
└────────┼───────────────┼───────────────┼───────────────┼────────┘
         │               │               │               │
         ▼               ▼               ▼               ▼
┌─────────────────────────────────────────────────────────────────┐
│                    Orchestrator Layer                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │ Task Manager │  │ Log Streamer │  │  Artifact    │          │
│  │              │  │              │  │  Collector   │          │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘          │
│         │                 │                 │                   │
└─────────┼─────────────────┼─────────────────┼────────────────────┘
          │                 │                 │
          ▼                 ▼                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Storage Layer                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │   SQLite DB  │  │ Log Ring     │  │  Filesystem  │          │
│  │   (Tasks)    │  │ Buffer       │  │  (Artifacts) │          │
│  └──────────────┘  └──────────────┘  └──────────────┘          │
└─────────────────────────────────────────────────────────────────┘
```

### 1.2 File Structure

```
management/src/
├── http/
│   ├── mod.rs              # HTTP server exports
│   ├── server.rs           # Axum server setup
│   └── tasks.rs            # Task API handlers (NEW)
│
├── orchestrator/
│   ├── mod.rs              # Orchestrator exports
│   ├── task.rs             # Task struct and state machine (EXISTS)
│   ├── manifest.rs         # Manifest parsing (EXISTS)
│   ├── storage.rs          # Task persistence (NEW)
│   ├── executor.rs         # Task execution logic (NEW)
│   ├── monitor.rs          # Progress monitoring (NEW)
│   ├── collector.rs        # Artifact collection (NEW)
│   └── secrets.rs          # Secret resolution (NEW)
│
└── ws/
    ├── mod.rs              # WebSocket exports
    ├── hub.rs              # WebSocket hub (EXISTS)
    └── task_events.rs      # Task event streaming (NEW)
```

---

## 2. Implementation Phases

### Phase 1: Core Task Management (Week 1)

**Goal:** Basic task submission and storage

**Tasks:**
1. Implement `management/src/orchestrator/storage.rs`
   - SQLite schema for tasks
   - CRUD operations
   - State transitions

2. Implement `management/src/http/tasks.rs`
   - `POST /api/v1/tasks` - Submit task
   - `GET /api/v1/tasks` - List tasks
   - `GET /api/v1/tasks/{id}` - Get task

3. Tests
   - Unit tests for storage layer
   - Integration tests for API endpoints

**Deliverables:**
- Task submission works
- Tasks persisted to database
- Tasks queryable via API

---

### Phase 2: Task Execution (Week 2)

**Goal:** Tasks actually execute Claude Code

**Tasks:**
1. Implement `management/src/orchestrator/executor.rs`
   - VM provisioning integration
   - Repository cloning
   - Claude Code invocation
   - Exit code handling

2. Implement `management/src/orchestrator/monitor.rs`
   - Progress tracking
   - Output capture
   - Heartbeat monitoring

3. Implement state transitions
   - pending → staging → provisioning → ready → running → completing → completed
   - Failure handling

**Deliverables:**
- End-to-end task execution
- State machine working correctly
- Progress updates in database

---

### Phase 3: Logging and Artifacts (Week 3)

**Goal:** Users can see what tasks are doing

**Tasks:**
1. Implement log streaming
   - `GET /api/v1/tasks/{id}/logs`
   - Ring buffer for recent logs
   - Follow mode (WebSocket)

2. Implement `management/src/orchestrator/collector.rs`
   - Glob pattern matching
   - Artifact copying
   - Checksum computation

3. Implement artifact endpoints
   - `GET /api/v1/tasks/{id}/artifacts`
   - `GET /api/v1/tasks/{id}/artifacts/{name}`

**Deliverables:**
- Real-time log streaming
- Artifact collection working
- Artifact download working

---

### Phase 4: WebSocket Events (Week 4)

**Goal:** Real-time task updates in web UI

**Tasks:**
1. Implement `management/src/ws/task_events.rs`
   - Event types (state_change, output, progress, metrics)
   - Subscription filtering
   - Event fanout

2. Integrate with orchestrator
   - Emit events on state change
   - Emit events on output
   - Emit events on progress update

3. Web UI integration
   - Subscribe to task events
   - Update UI in real-time

**Deliverables:**
- WebSocket event streaming
- Real-time UI updates
- Event filtering

---

### Phase 5: CLI (Week 5)

**Goal:** Command-line interface for tasks

**Tasks:**
1. Implement `cli/src/commands/task.rs`
   - `sandbox task submit`
   - `sandbox task list`
   - `sandbox task status`
   - `sandbox task logs`
   - `sandbox task cancel`
   - `sandbox task artifacts`

2. Output formatting
   - Text (human-friendly)
   - JSON (machine-parsable)
   - Table (list views)

**Deliverables:**
- Full CLI implementation
- Integration tests
- Documentation

---

### Phase 6: Polish and Hardening (Week 6)

**Goal:** Production-ready

**Tasks:**
1. Error handling
   - Comprehensive error types
   - User-friendly error messages
   - Trace IDs for debugging

2. Performance optimization
   - Database indexing
   - Query optimization
   - Caching

3. Documentation
   - API reference
   - User guide
   - Integration examples

**Deliverables:**
- Production-ready API
- Complete documentation
- Load testing results

---

## 3. Data Models

### 3.1 Database Schema (SQLite)

```sql
-- Tasks table
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    labels TEXT NOT NULL,  -- JSON object

    -- Repository config
    repo_url TEXT NOT NULL,
    repo_branch TEXT NOT NULL,
    repo_commit TEXT,
    repo_subpath TEXT,

    -- Claude config
    claude_prompt TEXT NOT NULL,
    claude_headless BOOLEAN NOT NULL DEFAULT 1,
    claude_skip_permissions BOOLEAN NOT NULL DEFAULT 1,
    claude_output_format TEXT NOT NULL DEFAULT 'stream-json',
    claude_model TEXT NOT NULL DEFAULT 'claude-sonnet-4-5-20250929',
    claude_allowed_tools TEXT,  -- JSON array
    claude_mcp_config TEXT,      -- JSON object
    claude_max_turns INTEGER,

    -- VM config
    vm_profile TEXT NOT NULL DEFAULT 'agentic-dev',
    vm_cpus INTEGER NOT NULL DEFAULT 4,
    vm_memory TEXT NOT NULL DEFAULT '8G',
    vm_disk TEXT NOT NULL DEFAULT '40G',
    vm_network_mode TEXT NOT NULL DEFAULT 'isolated',
    vm_allowed_hosts TEXT,       -- JSON array

    -- Lifecycle config
    lifecycle_timeout TEXT NOT NULL DEFAULT '24h',
    lifecycle_failure_action TEXT NOT NULL DEFAULT 'destroy',
    lifecycle_artifact_patterns TEXT,  -- JSON array

    -- Secrets (references only, NOT values)
    secrets TEXT,                -- JSON array of SecretRef

    -- Runtime state
    state TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    state_changed_at INTEGER NOT NULL,
    state_message TEXT,

    -- VM info (set when provisioned)
    vm_name TEXT,
    vm_ip TEXT,

    -- Completion info
    exit_code INTEGER,
    error TEXT,

    -- Progress
    progress_output_bytes INTEGER NOT NULL DEFAULT 0,
    progress_tool_calls INTEGER NOT NULL DEFAULT 0,
    progress_current_tool TEXT,
    progress_last_activity_at INTEGER
);

-- Indexes for common queries
CREATE INDEX idx_tasks_state ON tasks(state);
CREATE INDEX idx_tasks_created_at ON tasks(created_at DESC);
CREATE INDEX idx_tasks_started_at ON tasks(started_at DESC);

-- Task artifacts table
CREATE TABLE artifacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    content_type TEXT NOT NULL,
    checksum TEXT NOT NULL,
    created_at INTEGER NOT NULL,

    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
    UNIQUE(task_id, name)
);

CREATE INDEX idx_artifacts_task_id ON artifacts(task_id);

-- Task output logs (ring buffer, last 10k lines per task)
CREATE TABLE task_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    stream TEXT NOT NULL,  -- 'stdout', 'stderr', 'event'
    data TEXT NOT NULL,

    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX idx_logs_task_timestamp ON task_logs(task_id, timestamp);

-- Log retention trigger (keep last 10k lines per task)
CREATE TRIGGER keep_recent_logs
AFTER INSERT ON task_logs
BEGIN
    DELETE FROM task_logs
    WHERE task_id = NEW.task_id
    AND id NOT IN (
        SELECT id FROM task_logs
        WHERE task_id = NEW.task_id
        ORDER BY timestamp DESC
        LIMIT 10000
    );
END;
```

### 3.2 Rust Structs

**Task Storage Model:**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Task row in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRow {
    pub id: String,
    pub name: String,
    pub labels: HashMap<String, String>,

    // Repository
    pub repo_url: String,
    pub repo_branch: String,
    pub repo_commit: Option<String>,
    pub repo_subpath: Option<String>,

    // Claude
    pub claude_prompt: String,
    pub claude_headless: bool,
    pub claude_skip_permissions: bool,
    pub claude_output_format: String,
    pub claude_model: String,
    pub claude_allowed_tools: Option<Vec<String>>,
    pub claude_mcp_config: Option<serde_json::Value>,
    pub claude_max_turns: Option<u32>,

    // VM
    pub vm_profile: String,
    pub vm_cpus: u32,
    pub vm_memory: String,
    pub vm_disk: String,
    pub vm_network_mode: NetworkMode,
    pub vm_allowed_hosts: Vec<String>,

    // Lifecycle
    pub lifecycle_timeout: String,
    pub lifecycle_failure_action: String,
    pub lifecycle_artifact_patterns: Vec<String>,

    // Secrets
    pub secrets: Vec<SecretRef>,

    // Runtime state
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub state_changed_at: DateTime<Utc>,
    pub state_message: Option<String>,

    pub vm_name: Option<String>,
    pub vm_ip: Option<String>,

    pub exit_code: Option<i32>,
    pub error: Option<String>,

    // Progress
    pub progress_output_bytes: u64,
    pub progress_tool_calls: u32,
    pub progress_current_tool: Option<String>,
    pub progress_last_activity_at: Option<DateTime<Utc>>,
}

impl TaskRow {
    /// Convert to public API Task struct
    pub fn into_task(self) -> Task {
        Task {
            id: self.id,
            name: self.name,
            labels: self.labels,
            repository: RepositoryConfig { /* ... */ },
            claude: ClaudeConfig { /* ... */ },
            vm: VmConfig { /* ... */ },
            secrets: self.secrets,
            lifecycle: LifecycleConfig { /* ... */ },
            state: self.state,
            created_at: self.created_at,
            started_at: self.started_at,
            state_changed_at: self.state_changed_at,
            state_message: self.state_message,
            vm_name: self.vm_name,
            vm_ip: self.vm_ip,
            exit_code: self.exit_code,
            error: self.error,
            progress: TaskProgress {
                output_bytes: self.progress_output_bytes,
                tool_calls: self.progress_tool_calls,
                current_tool: self.progress_current_tool,
                last_activity_at: self.progress_last_activity_at,
            },
        }
    }
}
```

---

## 4. HTTP Handlers Implementation

### 4.1 Submit Task Handler

**File:** `management/src/http/tasks.rs`

```rust
use axum::{
    extract::{State, ContentType},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::orchestrator::{Orchestrator, TaskManifest};
use super::server::AppState;

/// POST /api/v1/tasks - Submit a new task
pub async fn submit_task(
    State(state): State<AppState>,
    content_type: ContentType,
    body: String,
) -> Result<impl IntoResponse, TaskError> {
    // Parse manifest based on content type
    let manifest = match content_type {
        ct if ct.as_ref() == "application/yaml" || ct.as_ref() == "text/yaml" => {
            TaskManifest::from_yaml(&body)
                .map_err(|e| TaskError::InvalidManifest(e.to_string()))?
        }
        ct if ct.as_ref() == "application/json" => {
            TaskManifest::from_json(&body)
                .map_err(|e| TaskError::InvalidManifest(e.to_string()))?
        }
        _ => {
            return Err(TaskError::UnsupportedContentType(content_type.to_string()));
        }
    };

    // Validate manifest
    manifest.validate()
        .map_err(|e| TaskError::InvalidManifest(e.to_string()))?;

    // Generate ID if empty
    let manifest = manifest.with_generated_id();

    // Submit to orchestrator
    let orchestrator = state.orchestrator
        .as_ref()
        .ok_or(TaskError::OrchestratorNotAvailable)?;

    let task = orchestrator.submit_task(manifest).await
        .map_err(|e| TaskError::SubmissionFailed(e.to_string()))?;

    // Return response
    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitTaskResponse {
            task_id: task.id.clone(),
            state: task.state.to_string(),
            message: "Task accepted and queued for execution".to_string(),
            created_at: task.created_at,
            links: TaskLinks {
                self_link: format!("/api/v1/tasks/{}", task.id),
                logs: format!("/api/v1/tasks/{}/logs", task.id),
                artifacts: format!("/api/v1/tasks/{}/artifacts", task.id),
            },
        }),
    ))
}

#[derive(Serialize)]
pub struct SubmitTaskResponse {
    pub task_id: String,
    pub state: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub links: TaskLinks,
}

#[derive(Serialize)]
pub struct TaskLinks {
    #[serde(rename = "self")]
    pub self_link: String,
    pub logs: String,
    pub artifacts: String,
}
```

### 4.2 List Tasks Handler

```rust
use axum::extract::Query;

#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    #[serde(default)]
    pub state: Option<String>,  // Comma-separated states

    #[serde(flatten)]
    pub labels: HashMap<String, String>,  // label.key=value

    #[serde(default = "default_limit")]
    pub limit: usize,

    #[serde(default)]
    pub offset: usize,

    #[serde(default = "default_sort")]
    pub sort: String,

    #[serde(default = "default_order")]
    pub order: String,
}

fn default_limit() -> usize { 100 }
fn default_sort() -> String { "created_at".to_string() }
fn default_order() -> String { "desc".to_string() }

/// GET /api/v1/tasks - List tasks
pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<ListTasksQuery>,
) -> Result<impl IntoResponse, TaskError> {
    let orchestrator = state.orchestrator
        .as_ref()
        .ok_or(TaskError::OrchestratorNotAvailable)?;

    // Parse state filter
    let state_filter = query.state.as_ref().map(|s| {
        s.split(',')
            .filter_map(|state_str| TaskState::from_str(state_str).ok())
            .collect::<Vec<_>>()
    });

    // Query tasks
    let tasks = orchestrator.list_tasks(state_filter, query.limit, query.offset).await
        .map_err(|e| TaskError::QueryFailed(e.to_string()))?;

    let total_count = orchestrator.count_tasks(state_filter).await
        .map_err(|e| TaskError::QueryFailed(e.to_string()))?;

    Ok(Json(ListTasksResponse {
        tasks: tasks.into_iter().map(TaskResponse::from).collect(),
        total_count,
        limit: query.limit,
        offset: query.offset,
        has_more: total_count > (query.offset + query.limit),
    }))
}

#[derive(Serialize)]
pub struct ListTasksResponse {
    pub tasks: Vec<TaskResponse>,
    pub total_count: usize,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}
```

### 4.3 Get Task Handler

```rust
use axum::extract::Path;

/// GET /api/v1/tasks/{id} - Get task status
pub async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, TaskError> {
    let orchestrator = state.orchestrator
        .as_ref()
        .ok_or(TaskError::OrchestratorNotAvailable)?;

    let task = orchestrator.get_task(&task_id).await
        .map_err(|e| TaskError::QueryFailed(e.to_string()))?
        .ok_or(TaskError::NotFound(task_id.clone()))?;

    Ok(Json(GetTaskResponse {
        id: task.id.clone(),
        name: task.name.clone(),
        state: task.state.to_string(),
        labels: task.labels.clone(),
        created_at: task.created_at,
        started_at: task.started_at,
        state_changed_at: task.state_changed_at,
        state_message: task.state_message.clone(),
        vm_name: task.vm_name.clone(),
        vm_ip: task.vm_ip.clone(),
        exit_code: task.exit_code,
        error: task.error.clone(),
        progress: task.progress.clone(),
        definition: TaskDefinitionResponse {
            repository: task.repository.clone(),
            claude: task.claude.clone(),
            vm: task.vm.clone(),
            lifecycle: task.lifecycle.clone(),
        },
    }))
}
```

### 4.4 Error Handling

```rust
use axum::response::IntoResponse;
use axum::http::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("Invalid manifest: {0}")]
    InvalidManifest(String),

    #[error("Unsupported content type: {0}")]
    UnsupportedContentType(String),

    #[error("Orchestrator not available")]
    OrchestratorNotAvailable,

    #[error("Task submission failed: {0}")]
    SubmissionFailed(String),

    #[error("Task not found: {0}")]
    NotFound(String),

    #[error("Query failed: {0}")]
    QueryFailed(String),

    #[error("Task already exists: {0}")]
    DuplicateTaskId(String),

    #[error("Task is in terminal state: {0}")]
    AlreadyTerminal(String),
}

impl IntoResponse for TaskError {
    fn into_response(self) -> Response {
        let (status, error_type, message) = match self {
            TaskError::InvalidManifest(msg) => {
                (StatusCode::BAD_REQUEST, "invalid_manifest", msg)
            }
            TaskError::UnsupportedContentType(msg) => {
                (StatusCode::BAD_REQUEST, "unsupported_content_type", msg)
            }
            TaskError::NotFound(msg) => {
                (StatusCode::NOT_FOUND, "task_not_found", msg)
            }
            TaskError::DuplicateTaskId(msg) => {
                (StatusCode::CONFLICT, "duplicate_task_id", msg)
            }
            TaskError::AlreadyTerminal(msg) => {
                (StatusCode::CONFLICT, "task_already_terminal", msg)
            }
            _ => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", self.to_string())
            }
        };

        let body = Json(serde_json::json!({
            "error": error_type,
            "message": message,
        }));

        (status, body).into_response()
    }
}
```

---

## 5. Orchestrator Implementation

### 5.1 Task Storage

**File:** `management/src/orchestrator/storage.rs`

```rust
use rusqlite::{Connection, params, OptionalExtension};
use std::sync::{Arc, Mutex};
use chrono::Utc;

use super::task::{Task, TaskState};

pub struct TaskStorage {
    conn: Arc<Mutex<Connection>>,
}

impl TaskStorage {
    pub fn new(db_path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(db_path)?;

        // Create tables
        conn.execute_batch(include_str!("../../../sql/schema.sql"))?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Insert a new task
    pub fn insert(&self, task: &Task) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        conn.execute(
            "INSERT INTO tasks (
                id, name, labels, repo_url, repo_branch, claude_prompt,
                state, created_at, state_changed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                task.id,
                task.name,
                serde_json::to_string(&task.labels).unwrap(),
                task.repository.url,
                task.repository.branch,
                task.claude.prompt,
                task.state.to_string(),
                task.created_at.timestamp(),
                task.state_changed_at.timestamp(),
            ],
        )?;

        Ok(())
    }

    /// Get task by ID
    pub fn get(&self, task_id: &str) -> Result<Option<Task>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let task = conn.query_row(
            "SELECT * FROM tasks WHERE id = ?1",
            params![task_id],
            |row| {
                // Map row to Task struct
                // (implementation omitted for brevity)
                Ok(Task { /* ... */ })
            },
        ).optional()?;

        Ok(task)
    }

    /// List tasks with filters
    pub fn list(
        &self,
        state_filter: Option<Vec<TaskState>>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Task>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        let mut sql = String::from("SELECT * FROM tasks");
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![];

        // Add state filter
        if let Some(states) = state_filter {
            let placeholders = states.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            sql.push_str(&format!(" WHERE state IN ({})", placeholders));
            for state in states {
                params.push(Box::new(state.to_string()));
            }
        }

        // Add ordering and pagination
        sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");
        params.push(Box::new(limit));
        params.push(Box::new(offset));

        let mut stmt = conn.prepare(&sql)?;
        let tasks = stmt.query_map(
            params.iter().map(|p| p.as_ref()).collect::<Vec<_>>().as_slice(),
            |row| {
                // Map row to Task struct
                Ok(Task { /* ... */ })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

        Ok(tasks)
    }

    /// Update task state
    pub fn update_state(
        &self,
        task_id: &str,
        new_state: TaskState,
        message: Option<String>,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now();

        conn.execute(
            "UPDATE tasks SET state = ?1, state_changed_at = ?2, state_message = ?3 WHERE id = ?4",
            params![new_state.to_string(), now.timestamp(), message, task_id],
        )?;

        Ok(())
    }
}
```

---

## 6. Testing

### 6.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_storage_insert_and_get() {
        let storage = TaskStorage::new(":memory:").unwrap();

        let task = Task {
            id: "test-001".to_string(),
            name: "Test Task".to_string(),
            // ... rest of fields
        };

        storage.insert(&task).unwrap();

        let retrieved = storage.get("test-001").unwrap().unwrap();
        assert_eq!(retrieved.id, "test-001");
        assert_eq!(retrieved.name, "Test Task");
    }

    #[tokio::test]
    async fn test_submit_task_api() {
        let manifest = r#"
version: "1"
kind: Task
metadata:
  name: "Test"
repository:
  url: "https://github.com/test/repo.git"
  branch: "main"
claude:
  prompt: "Test"
"#;

        // Setup test server
        let app = create_test_app().await;

        // Submit task
        let response = app
            .post("/api/v1/tasks")
            .header("Content-Type", "application/yaml")
            .body(manifest)
            .send()
            .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let body: SubmitTaskResponse = response.json().await;
        assert!(!body.task_id.is_empty());
        assert_eq!(body.state, "pending");
    }
}
```

### 6.2 Integration Tests

**File:** `tests/e2e/test_task_api.py`

```python
import pytest
import requests
import yaml

def test_submit_task(management_server):
    """Test task submission"""
    manifest = {
        "version": "1",
        "kind": "Task",
        "metadata": {"name": "Test Task"},
        "repository": {
            "url": "https://github.com/test/repo.git",
            "branch": "main"
        },
        "claude": {"prompt": "Test"},
    }

    response = requests.post(
        f"{management_server}/api/v1/tasks",
        json=manifest
    )

    assert response.status_code == 202
    data = response.json()
    assert "task_id" in data
    assert data["state"] == "pending"

    task_id = data["task_id"]

    # Get task status
    response = requests.get(f"{management_server}/api/v1/tasks/{task_id}")
    assert response.status_code == 200
    data = response.json()
    assert data["id"] == task_id
    assert data["name"] == "Test Task"

def test_list_tasks(management_server):
    """Test listing tasks"""
    response = requests.get(f"{management_server}/api/v1/tasks")
    assert response.status_code == 200
    data = response.json()
    assert "tasks" in data
    assert "total_count" in data
```

---

## 7. Performance Considerations

### 7.1 Database Indexing

- Index on `state` for filtering
- Index on `created_at` for sorting
- Compound index on `(state, created_at)` for common query

### 7.2 Caching

```rust
use moka::future::Cache;
use std::time::Duration;

pub struct CachedTaskStorage {
    storage: TaskStorage,
    cache: Cache<String, Task>,
}

impl CachedTaskStorage {
    pub fn new(storage: TaskStorage) -> Self {
        let cache = Cache::builder()
            .time_to_live(Duration::from_secs(5))
            .max_capacity(1000)
            .build();

        Self { storage, cache }
    }

    pub async fn get(&self, task_id: &str) -> Result<Option<Task>, Error> {
        // Try cache first
        if let Some(task) = self.cache.get(task_id).await {
            return Ok(Some(task));
        }

        // Cache miss, query database
        let task = self.storage.get(task_id)?;

        // Store in cache
        if let Some(ref t) = task {
            self.cache.insert(task_id.to_string(), t.clone()).await;
        }

        Ok(task)
    }
}
```

### 7.3 Concurrency

```rust
use tokio::sync::RwLock;

pub struct Orchestrator {
    storage: Arc<RwLock<TaskStorage>>,
    executor: Arc<TaskExecutor>,
}

impl Orchestrator {
    pub async fn submit_task(&self, manifest: TaskManifest) -> Result<Task, Error> {
        let task = Task::from_manifest(manifest)?;

        // Write lock for insert
        {
            let storage = self.storage.write().await;
            storage.insert(&task)?;
        }

        // Spawn execution task (don't block)
        let executor = Arc::clone(&self.executor);
        let task_id = task.id.clone();
        tokio::spawn(async move {
            executor.execute(task_id).await;
        });

        Ok(task)
    }
}
```

---

## 8. Next Steps

### Immediate (Week 1)
1. Implement `TaskStorage` with SQLite
2. Implement `submit_task` and `get_task` handlers
3. Write unit tests for storage layer

### Short Term (Weeks 2-3)
1. Implement task executor
2. Implement log streaming
3. Implement artifact collection

### Medium Term (Weeks 4-6)
1. WebSocket event streaming
2. CLI implementation
3. Integration tests and documentation

---

## Appendix: SQL Schema

**File:** `management/sql/schema.sql`

See Section 3.1 for complete schema.

---

**Document Metadata:**
- Version: 1.0.0
- Date: 2026-01-29
- Author: API Designer
- Audience: Software Implementers
- Status: Implementation Guide

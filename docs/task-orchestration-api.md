# Task Orchestration API Specification

Version: 1.0.0
Date: 2026-01-29

## Overview

Complete API specification for task submission, monitoring, and control in the agentic-sandbox system. This document covers the YAML task manifest format, REST API endpoints, WebSocket events, and CLI commands.

---

## 1. Task Manifest Format

### 1.1 Complete YAML Schema

```yaml
# Task manifest version (required)
version: "1"

# Resource kind (required, must be "Task")
kind: Task

# Metadata section (required)
metadata:
  # Unique task identifier (required, must be unique across all tasks)
  # If empty, server will generate a UUIDv4
  id: "task-12345"

  # Human-readable task name (required)
  name: "Fix authentication bug in API"

  # Custom labels for organization and filtering (optional)
  labels:
    team: "platform"
    priority: "high"
    project: "auth-service"
    sprint: "2026-02"

# Repository configuration (required)
repository:
  # Git repository URL (required)
  # Supports HTTPS and SSH formats
  url: "https://github.com/org/repo.git"
  # or: "git@github.com:org/repo.git"

  # Branch name (required)
  branch: "main"

  # Optional: Pin to specific commit SHA
  # If omitted, uses latest commit on branch
  commit: "abc123def456"

  # Optional: Work in a subdirectory of the repo
  # Useful for monorepos
  subpath: "services/api"

# Claude Code configuration (required)
claude:
  # Task prompt/instructions (required)
  # This is what Claude will be asked to do
  prompt: |
    Review the authentication bug report in GitHub issue #42.
    Analyze the code, identify the root cause, and implement a fix.
    Add comprehensive tests for the fix.
    Create a summary report of the issue and solution in REPORT.md.

  # Run in headless mode without user interaction (default: true)
  headless: true

  # Skip permission prompts (default: true)
  # Use --dangerously-skip-permissions flag
  skip_permissions: true

  # Output format (default: "stream-json")
  # Options: "stream-json", "json", "text"
  output_format: "stream-json"

  # Claude model to use (default: "claude-sonnet-4-5-20250929")
  model: "claude-sonnet-4-5-20250929"

  # Allowed tools (optional, if omitted all tools are allowed)
  # Restricts which tools Claude can use
  allowed_tools:
    - "Read"
    - "Write"
    - "Edit"
    - "Bash"
    - "Glob"
    - "Grep"

  # MCP (Model Context Protocol) configuration (optional)
  # Provided as inline JSON
  mcp_config:
    mcpServers:
      github:
        command: "npx"
        args: ["-y", "@modelcontextprotocol/server-github"]
        env:
          GITHUB_TOKEN: "${GITHUB_TOKEN}"

  # Maximum conversation turns (optional)
  # Limits the length of Claude's execution
  max_turns: 100

# VM resource configuration (optional, defaults shown)
vm:
  # VM profile name (default: "agentic-dev")
  # Options: "agentic-dev", "basic"
  profile: "agentic-dev"

  # CPU cores (default: 4)
  cpus: 8

  # Memory allocation (default: "8G")
  # Format: <number>G (gigabytes) or <number>M (megabytes)
  memory: "16G"

  # Disk size (default: "40G")
  # Format: <number>G (gigabytes)
  disk: "50G"

  # Network isolation mode (default: "isolated")
  # Options:
  #   - "isolated": No network access
  #   - "outbound": Outbound connections only to allowed_hosts
  #   - "full": Full network access
  network_mode: "outbound"

  # Allowed hosts for outbound mode (optional)
  # Only used when network_mode is "outbound"
  allowed_hosts:
    - "api.github.com"
    - "pypi.org"
    - "registry.npmjs.org"

# Secret references (optional)
# Secrets are resolved at orchestration time and injected into VM
secrets:
  # Environment variable secret
  - name: "ANTHROPIC_API_KEY"
    source: "env"
    key: "ANTHROPIC_API_KEY"

  # Vault secret (requires Vault integration)
  - name: "GITHUB_TOKEN"
    source: "vault"
    key: "github/tokens/ci"

  # File-based secret
  - name: "SSH_PRIVATE_KEY"
    source: "file"
    key: "/etc/agentic-sandbox/secrets/deploy-key"

# Lifecycle configuration (optional, defaults shown)
lifecycle:
  # Task timeout (default: "24h")
  # Format: <number>h (hours), <number>m (minutes), or <number>s (seconds)
  # After this time, task will be cancelled
  timeout: "6h"

  # What to do on failure (default: "destroy")
  # Options:
  #   - "destroy": Destroy VM immediately
  #   - "preserve": Keep VM running for debugging
  failure_action: "preserve"

  # Artifact collection patterns (optional)
  # Glob patterns relative to workspace root
  # Matched files will be collected to /mnt/inbox/<task-id>/
  artifact_patterns:
    - "*.patch"
    - "REPORT.md"
    - "reports/*.json"
    - "build/dist/**/*"
```

### 1.2 Minimal Example

Simplest possible task manifest:

```yaml
version: "1"
kind: Task
metadata:
  id: ""  # Server will generate UUID
  name: "Quick Test"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Run the test suite and report results"
```

All other fields use defaults:
- VM: 4 CPUs, 8G RAM, 40G disk, agentic-dev profile
- Claude: Headless, skip permissions, stream-json output, Sonnet 4.5
- Lifecycle: 24h timeout, destroy on failure
- Network: Isolated (no network access)

### 1.3 Common Scenarios

#### Scenario 1: Refactoring Task

```yaml
version: "1"
kind: Task
metadata:
  id: "refactor-auth-001"
  name: "Refactor authentication module"
  labels:
    type: "refactoring"
    priority: "medium"
repository:
  url: "git@github.com:company/auth-service.git"
  branch: "develop"
  subpath: "src/auth"
claude:
  prompt: |
    Refactor the authentication module to use dependency injection.
    Update all tests to use mocks.
    Maintain 100% backward compatibility.
  model: "claude-opus-4-5-20251101"  # Use Opus for complex refactoring
  max_turns: 200
vm:
  cpus: 8
  memory: "16G"
  network_mode: "outbound"
  allowed_hosts:
    - "pypi.org"  # For installing test dependencies
lifecycle:
  timeout: "12h"
  failure_action: "preserve"
  artifact_patterns:
    - "refactoring-report.md"
    - "test-results.xml"
```

#### Scenario 2: Bug Fix with GitHub Integration

```yaml
version: "1"
kind: Task
metadata:
  id: "bugfix-gh-42"
  name: "Fix issue #42"
  labels:
    type: "bugfix"
    issue: "42"
repository:
  url: "https://github.com/org/app.git"
  branch: "main"
claude:
  prompt: "Fix the bug described in GitHub issue #42. Add tests and create a PR."
  mcp_config:
    mcpServers:
      github:
        command: "npx"
        args: ["-y", "@modelcontextprotocol/server-github"]
        env:
          GITHUB_TOKEN: "${GITHUB_TOKEN}"
vm:
  network_mode: "outbound"
  allowed_hosts:
    - "api.github.com"
secrets:
  - name: "GITHUB_TOKEN"
    source: "env"
    key: "GITHUB_TOKEN"
  - name: "ANTHROPIC_API_KEY"
    source: "env"
    key: "ANTHROPIC_API_KEY"
lifecycle:
  timeout: "4h"
  artifact_patterns:
    - "*.patch"
```

#### Scenario 3: Security Audit

```yaml
version: "1"
kind: Task
metadata:
  id: "security-audit-q1-2026"
  name: "Q1 2026 Security Audit"
  labels:
    type: "security"
    quarter: "2026-Q1"
repository:
  url: "https://github.com/org/backend.git"
  branch: "production"
claude:
  prompt: |
    Perform a comprehensive security audit:
    1. Run static analysis tools (bandit, semgrep)
    2. Review authentication and authorization code
    3. Check for known vulnerable dependencies
    4. Generate a detailed security report with findings
  allowed_tools:
    - "Read"
    - "Bash"
    - "Grep"
    - "Write"
  model: "claude-opus-4-5-20251101"
vm:
  cpus: 4
  memory: "8G"
  network_mode: "isolated"  # No network for security audit
lifecycle:
  timeout: "8h"
  artifact_patterns:
    - "security-report.md"
    - "findings/*.json"
```

#### Scenario 4: Data Processing Task

```yaml
version: "1"
kind: Task
metadata:
  id: "process-dataset-20260129"
  name: "Process customer dataset"
  labels:
    type: "data-processing"
    date: "2026-01-29"
repository:
  url: "https://github.com/org/data-pipeline.git"
  branch: "main"
  subpath: "processors"
claude:
  prompt: |
    Process the customer dataset in /mnt/global/datasets/customers.csv:
    1. Clean and normalize data
    2. Generate summary statistics
    3. Create visualization reports
    4. Output processed data to /mnt/inbox
  headless: true
  skip_permissions: true
vm:
  cpus: 16
  memory: "32G"
  disk: "100G"
  profile: "agentic-dev"
lifecycle:
  timeout: "24h"
  artifact_patterns:
    - "processed-data.csv"
    - "reports/**/*"
    - "visualizations/*.png"
```

---

## 2. REST API Endpoints

Base URL: `http://localhost:8122/api/v1`

### 2.1 Submit Task

Create and submit a new task for execution.

**Endpoint:** `POST /api/v1/tasks`

**Request Headers:**
```
Content-Type: application/yaml
# or: Content-Type: application/json
```

**Request Body (YAML):**
```yaml
version: "1"
kind: Task
metadata:
  id: "my-task-123"
  name: "Example Task"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Run tests and fix any failures"
```

**Request Body (JSON):**
```json
{
  "version": "1",
  "kind": "Task",
  "metadata": {
    "id": "my-task-123",
    "name": "Example Task"
  },
  "repository": {
    "url": "https://github.com/example/repo.git",
    "branch": "main"
  },
  "claude": {
    "prompt": "Run tests and fix any failures"
  }
}
```

**Success Response:**
```json
HTTP/1.1 202 Accepted
Content-Type: application/json

{
  "task_id": "my-task-123",
  "state": "pending",
  "message": "Task accepted and queued for execution",
  "created_at": "2026-01-29T10:30:00Z",
  "links": {
    "self": "/api/v1/tasks/my-task-123",
    "logs": "/api/v1/tasks/my-task-123/logs",
    "artifacts": "/api/v1/tasks/my-task-123/artifacts"
  }
}
```

**Error Responses:**

```json
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
  "error": "invalid_manifest",
  "message": "Missing required field: claude.prompt",
  "details": {
    "field": "claude.prompt",
    "validation": "required"
  }
}
```

```json
HTTP/1.1 409 Conflict
Content-Type: application/json

{
  "error": "duplicate_task_id",
  "message": "Task with ID 'my-task-123' already exists",
  "existing_task": {
    "id": "my-task-123",
    "state": "running",
    "created_at": "2026-01-29T09:00:00Z"
  }
}
```

```json
HTTP/1.1 507 Insufficient Storage
Content-Type: application/json

{
  "error": "insufficient_resources",
  "message": "Not enough resources to provision VM",
  "details": {
    "requested_cpus": 16,
    "available_cpus": 8,
    "requested_memory": "32G",
    "available_memory": "16G"
  }
}
```

---

### 2.2 List Tasks

Retrieve a list of tasks with optional filtering.

**Endpoint:** `GET /api/v1/tasks`

**Query Parameters:**
- `state` (optional): Filter by state (comma-separated for multiple)
  - Values: `pending`, `staging`, `provisioning`, `ready`, `running`, `completing`, `completed`, `failed`, `failed_preserved`, `cancelled`
- `label.<key>` (optional): Filter by label (e.g., `label.team=platform`)
- `limit` (optional): Maximum results (default: 100, max: 1000)
- `offset` (optional): Pagination offset (default: 0)
- `sort` (optional): Sort field (default: `created_at`)
  - Values: `created_at`, `started_at`, `state_changed_at`, `name`
- `order` (optional): Sort order (default: `desc`)
  - Values: `asc`, `desc`

**Example Requests:**

```bash
# List all tasks
GET /api/v1/tasks

# List running tasks
GET /api/v1/tasks?state=running

# List failed or cancelled tasks
GET /api/v1/tasks?state=failed,cancelled

# List tasks for team "platform"
GET /api/v1/tasks?label.team=platform

# Paginated list
GET /api/v1/tasks?limit=50&offset=100

# List completed tasks, newest first
GET /api/v1/tasks?state=completed&sort=created_at&order=desc
```

**Success Response:**

```json
HTTP/1.1 200 OK
Content-Type: application/json

{
  "tasks": [
    {
      "id": "task-001",
      "name": "Fix auth bug",
      "state": "running",
      "labels": {
        "team": "platform",
        "priority": "high"
      },
      "created_at": "2026-01-29T10:00:00Z",
      "started_at": "2026-01-29T10:05:00Z",
      "state_changed_at": "2026-01-29T10:10:00Z",
      "state_message": "Claude Code executing",
      "vm_name": "task-001-vm",
      "vm_ip": "192.168.122.100",
      "progress": {
        "output_bytes": 45678,
        "tool_calls": 23,
        "current_tool": "Bash",
        "last_activity_at": "2026-01-29T10:25:30Z"
      }
    },
    {
      "id": "task-002",
      "name": "Security audit",
      "state": "completed",
      "labels": {
        "team": "security",
        "type": "audit"
      },
      "created_at": "2026-01-29T08:00:00Z",
      "started_at": "2026-01-29T08:02:00Z",
      "state_changed_at": "2026-01-29T09:45:00Z",
      "state_message": "Task completed successfully",
      "vm_name": null,
      "vm_ip": null,
      "exit_code": 0,
      "progress": {
        "output_bytes": 123456,
        "tool_calls": 67,
        "current_tool": null,
        "last_activity_at": "2026-01-29T09:44:55Z"
      }
    }
  ],
  "total_count": 156,
  "limit": 100,
  "offset": 0,
  "has_more": true
}
```

---

### 2.3 Get Task Status

Retrieve detailed status for a specific task.

**Endpoint:** `GET /api/v1/tasks/{id}`

**Path Parameters:**
- `id` (required): Task ID

**Example Request:**

```bash
GET /api/v1/tasks/task-001
```

**Success Response:**

```json
HTTP/1.1 200 OK
Content-Type: application/json

{
  "id": "task-001",
  "name": "Fix auth bug",
  "state": "running",
  "labels": {
    "team": "platform",
    "priority": "high"
  },
  "created_at": "2026-01-29T10:00:00Z",
  "started_at": "2026-01-29T10:05:00Z",
  "state_changed_at": "2026-01-29T10:10:00Z",
  "state_message": "Claude Code executing",
  "vm_name": "task-001-vm",
  "vm_ip": "192.168.122.100",
  "progress": {
    "output_bytes": 45678,
    "tool_calls": 23,
    "current_tool": "Bash",
    "last_activity_at": "2026-01-29T10:25:30Z"
  },
  "definition": {
    "repository": {
      "url": "https://github.com/org/app.git",
      "branch": "main",
      "commit": null,
      "subpath": null
    },
    "claude": {
      "prompt": "Fix the authentication bug in issue #42",
      "headless": true,
      "skip_permissions": true,
      "output_format": "stream-json",
      "model": "claude-sonnet-4-5-20250929",
      "allowed_tools": [],
      "mcp_config": null,
      "max_turns": null
    },
    "vm": {
      "profile": "agentic-dev",
      "cpus": 4,
      "memory": "8G",
      "disk": "40G",
      "network_mode": "outbound",
      "allowed_hosts": ["api.github.com"]
    },
    "lifecycle": {
      "timeout": "4h",
      "failure_action": "destroy",
      "artifact_patterns": ["*.patch"]
    }
  }
}
```

**Error Response:**

```json
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "error": "task_not_found",
  "message": "Task 'task-999' not found"
}
```

---

### 2.4 Cancel Task

Cancel a running or pending task.

**Endpoint:** `DELETE /api/v1/tasks/{id}`

**Path Parameters:**
- `id` (required): Task ID

**Request Body (optional):**

```json
{
  "reason": "Duplicate task was submitted",
  "force": false
}
```

**Query Parameters:**
- `reason` (optional): Cancellation reason
- `force` (optional): Force immediate termination (default: false)

**Example Request:**

```bash
DELETE /api/v1/tasks/task-001?reason=User+requested+cancellation
```

**Success Response:**

```json
HTTP/1.1 200 OK
Content-Type: application/json

{
  "success": true,
  "task_id": "task-001",
  "previous_state": "running",
  "new_state": "cancelled",
  "message": "Task cancelled successfully",
  "cancelled_at": "2026-01-29T10:30:00Z"
}
```

**Error Responses:**

```json
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "error": "task_not_found",
  "message": "Task 'task-999' not found"
}
```

```json
HTTP/1.1 409 Conflict
Content-Type: application/json

{
  "error": "task_already_terminal",
  "message": "Task is already in terminal state 'completed'",
  "task_state": "completed"
}
```

---

### 2.5 Stream Task Logs

Stream task output logs in real-time.

**Endpoint:** `GET /api/v1/tasks/{id}/logs`

**Path Parameters:**
- `id` (required): Task ID

**Query Parameters:**
- `follow` (optional): Follow logs in real-time (default: false)
- `stream` (optional): Stream type filter (default: all)
  - Values: `all`, `stdout`, `stderr`, `events`
- `since` (optional): Show logs since timestamp (ISO 8601)
- `tail` (optional): Show last N lines (default: 0 = all)

**Example Requests:**

```bash
# Get all logs
GET /api/v1/tasks/task-001/logs

# Follow logs in real-time
GET /api/v1/tasks/task-001/logs?follow=true

# Get only stdout
GET /api/v1/tasks/task-001/logs?stream=stdout

# Get last 100 lines and follow
GET /api/v1/tasks/task-001/logs?tail=100&follow=true

# Get logs since timestamp
GET /api/v1/tasks/task-001/logs?since=2026-01-29T10:00:00Z
```

**Success Response (text stream):**

```
HTTP/1.1 200 OK
Content-Type: text/plain; charset=utf-8
Transfer-Encoding: chunked

[2026-01-29T10:10:15Z] [STDOUT] Cloning repository...
[2026-01-29T10:10:17Z] [STDOUT] Repository cloned successfully
[2026-01-29T10:10:18Z] [EVENT] {"type":"tool_call","tool":"Read","file":"/app/src/auth.py"}
[2026-01-29T10:10:20Z] [STDOUT] Reading authentication module...
[2026-01-29T10:10:22Z] [EVENT] {"type":"tool_call","tool":"Bash","command":"pytest tests/test_auth.py"}
[2026-01-29T10:10:25Z] [STDOUT] Running tests...
[2026-01-29T10:10:30Z] [STDERR] FAILED tests/test_auth.py::test_login - AssertionError
```

**Success Response (JSON stream with follow=true):**

```
HTTP/1.1 200 OK
Content-Type: application/x-ndjson
Transfer-Encoding: chunked

{"timestamp":"2026-01-29T10:10:15Z","stream":"stdout","data":"Cloning repository...\n"}
{"timestamp":"2026-01-29T10:10:17Z","stream":"stdout","data":"Repository cloned successfully\n"}
{"timestamp":"2026-01-29T10:10:18Z","stream":"event","type":"tool_call","tool":"Read","file":"/app/src/auth.py"}
{"timestamp":"2026-01-29T10:10:20Z","stream":"stdout","data":"Reading authentication module...\n"}
```

**Error Response:**

```json
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "error": "task_not_found",
  "message": "Task 'task-999' not found"
}
```

---

### 2.6 List Artifacts

List all artifacts collected from a task.

**Endpoint:** `GET /api/v1/tasks/{id}/artifacts`

**Path Parameters:**
- `id` (required): Task ID

**Example Request:**

```bash
GET /api/v1/tasks/task-001/artifacts
```

**Success Response:**

```json
HTTP/1.1 200 OK
Content-Type: application/json

{
  "task_id": "task-001",
  "artifacts": [
    {
      "name": "fix-auth-bug.patch",
      "path": "fix-auth-bug.patch",
      "size_bytes": 4567,
      "content_type": "text/x-patch",
      "checksum": "sha256:abc123...",
      "created_at": "2026-01-29T10:45:00Z",
      "download_url": "/api/v1/tasks/task-001/artifacts/fix-auth-bug.patch"
    },
    {
      "name": "REPORT.md",
      "path": "REPORT.md",
      "size_bytes": 2345,
      "content_type": "text/markdown",
      "checksum": "sha256:def456...",
      "created_at": "2026-01-29T10:45:00Z",
      "download_url": "/api/v1/tasks/task-001/artifacts/REPORT.md"
    },
    {
      "name": "test-results.xml",
      "path": "reports/test-results.xml",
      "size_bytes": 12345,
      "content_type": "application/xml",
      "checksum": "sha256:789abc...",
      "created_at": "2026-01-29T10:45:00Z",
      "download_url": "/api/v1/tasks/task-001/artifacts/test-results.xml"
    }
  ],
  "total_count": 3,
  "total_size_bytes": 19257
}
```

**Error Response:**

```json
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "error": "task_not_found",
  "message": "Task 'task-999' not found"
}
```

---

### 2.7 Download Artifact

Download a specific artifact from a task.

**Endpoint:** `GET /api/v1/tasks/{id}/artifacts/{name}`

**Path Parameters:**
- `id` (required): Task ID
- `name` (required): Artifact name (URL-encoded)

**Example Request:**

```bash
GET /api/v1/tasks/task-001/artifacts/fix-auth-bug.patch
```

**Success Response:**

```
HTTP/1.1 200 OK
Content-Type: text/x-patch
Content-Length: 4567
Content-Disposition: attachment; filename="fix-auth-bug.patch"
X-Checksum-SHA256: abc123...

diff --git a/src/auth.py b/src/auth.py
index 123..456 789
--- a/src/auth.py
+++ b/src/auth.py
...
```

**Error Responses:**

```json
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "error": "artifact_not_found",
  "message": "Artifact 'missing.txt' not found for task 'task-001'"
}
```

---

## 3. WebSocket Events

WebSocket endpoint for real-time task updates.

**Endpoint:** `ws://localhost:8121/api/v1/tasks/stream`

**Query Parameters:**
- `task_id` (optional): Subscribe to specific task
- `filter` (optional): Event type filter (comma-separated)
  - Values: `state_change`, `output`, `progress`, `metrics`

### 3.1 Connection Setup

```javascript
const ws = new WebSocket('ws://localhost:8121/api/v1/tasks/stream?task_id=task-001');

ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log('Event:', data);
};
```

### 3.2 Event Types

#### State Change Event

Sent when task transitions to a new state.

```json
{
  "type": "state_change",
  "task_id": "task-001",
  "timestamp": "2026-01-29T10:10:00Z",
  "previous_state": "provisioning",
  "new_state": "running",
  "state_message": "Claude Code executing"
}
```

#### Output Event

Sent when task produces output.

```json
{
  "type": "output",
  "task_id": "task-001",
  "timestamp": "2026-01-29T10:10:15Z",
  "stream": "stdout",
  "data": "Cloning repository...\n",
  "bytes": 23
}
```

#### Progress Event

Sent periodically with execution progress.

```json
{
  "type": "progress",
  "task_id": "task-001",
  "timestamp": "2026-01-29T10:15:00Z",
  "progress": {
    "output_bytes": 45678,
    "tool_calls": 23,
    "current_tool": "Bash",
    "last_activity_at": "2026-01-29T10:14:55Z"
  }
}
```

#### Metrics Event

Sent with VM resource metrics.

```json
{
  "type": "metrics",
  "task_id": "task-001",
  "timestamp": "2026-01-29T10:15:00Z",
  "metrics": {
    "cpu_percent": 45.2,
    "memory_used_bytes": 4294967296,
    "memory_total_bytes": 8589934592,
    "disk_used_bytes": 5368709120,
    "disk_total_bytes": 42949672960,
    "load_avg": [1.5, 1.2, 0.9]
  }
}
```

#### Error Event

Sent when an error occurs.

```json
{
  "type": "error",
  "task_id": "task-001",
  "timestamp": "2026-01-29T10:20:00Z",
  "error": "command_failed",
  "message": "Claude Code exited with code 1",
  "details": {
    "exit_code": 1,
    "stderr": "Error: Failed to authenticate..."
  }
}
```

---

## 4. CLI Commands

Unified CLI for task management.

### 4.1 Submit Task

Submit a task manifest for execution.

```bash
sandbox task submit <manifest.yaml>
sandbox task submit <manifest.json>
```

**Options:**
- `--server, -s <url>`: Management server URL (default: http://localhost:8122)
- `--format, -f <format>`: Output format: text, json (default: text)
- `--wait, -w`: Wait for task completion
- `--follow`: Follow logs after submission

**Examples:**

```bash
# Submit task
sandbox task submit task.yaml

# Submit and wait for completion
sandbox task submit task.yaml --wait

# Submit and follow logs
sandbox task submit task.yaml --follow

# Submit with JSON output
sandbox task submit task.yaml --format json

# Submit to remote server
sandbox task submit task.yaml --server https://sandbox.example.com
```

**Output (text):**

```
Task submitted successfully
  ID: task-001
  Name: Fix auth bug
  State: pending
  Created: 2026-01-29 10:00:00 UTC

View status: sandbox task status task-001
Follow logs: sandbox task logs task-001 --follow
```

**Output (JSON):**

```json
{
  "task_id": "task-001",
  "state": "pending",
  "message": "Task accepted and queued for execution",
  "created_at": "2026-01-29T10:00:00Z"
}
```

---

### 4.2 List Tasks

List all tasks with optional filtering.

```bash
sandbox task list [options]
```

**Options:**
- `--state, -s <state>`: Filter by state (comma-separated)
- `--label <key=value>`: Filter by label
- `--limit <n>`: Maximum results (default: 100)
- `--format, -f <format>`: Output format: text, json, table (default: table)

**Examples:**

```bash
# List all tasks
sandbox task list

# List running tasks
sandbox task list --state running

# List failed or cancelled tasks
sandbox task list --state failed,cancelled

# List tasks for team "platform"
sandbox task list --label team=platform

# List with JSON output
sandbox task list --format json
```

**Output (table):**

```
ID          NAME                    STATE      CREATED              STARTED
task-001    Fix auth bug           running    2026-01-29 10:00     2026-01-29 10:05
task-002    Security audit         completed  2026-01-29 08:00     2026-01-29 08:02
task-003    Refactor module        failed     2026-01-29 09:00     2026-01-29 09:03
```

---

### 4.3 Get Task Status

Get detailed status for a specific task.

```bash
sandbox task status <task-id>
```

**Options:**
- `--format, -f <format>`: Output format: text, json (default: text)
- `--watch, -w`: Watch for status changes

**Examples:**

```bash
# Get status
sandbox task status task-001

# Watch status updates
sandbox task status task-001 --watch

# Get JSON output
sandbox task status task-001 --format json
```

**Output (text):**

```
Task: task-001
Name: Fix auth bug
State: running
Message: Claude Code executing

Created: 2026-01-29 10:00:00 UTC
Started: 2026-01-29 10:05:00 UTC
Updated: 2026-01-29 10:10:00 UTC

VM: task-001-vm (192.168.122.100)
Profile: agentic-dev
Resources: 4 CPUs, 8G RAM

Progress:
  Output: 44.6 KB
  Tool calls: 23
  Current tool: Bash
  Last activity: 2026-01-29 10:25:30 UTC

Repository: https://github.com/org/app.git
Branch: main
```

---

### 4.4 Stream Task Logs

Stream logs from a running or completed task.

```bash
sandbox task logs <task-id> [options]
```

**Options:**
- `--follow, -f`: Follow logs in real-time
- `--tail <n>`: Show last N lines (default: all)
- `--stream <type>`: Filter stream type: all, stdout, stderr, events (default: all)
- `--since <timestamp>`: Show logs since timestamp
- `--timestamps`: Show timestamps

**Examples:**

```bash
# View all logs
sandbox task logs task-001

# Follow logs in real-time
sandbox task logs task-001 --follow

# Show last 100 lines and follow
sandbox task logs task-001 --tail 100 --follow

# Show only stdout
sandbox task logs task-001 --stream stdout

# Show logs since timestamp
sandbox task logs task-001 --since "2026-01-29 10:00:00"

# Show with timestamps
sandbox task logs task-001 --timestamps
```

**Output:**

```
Cloning repository...
Repository cloned successfully
Reading authentication module...
Running tests...
FAILED tests/test_auth.py::test_login - AssertionError
Analyzing failure...
```

**Output (with timestamps):**

```
[2026-01-29 10:10:15] Cloning repository...
[2026-01-29 10:10:17] Repository cloned successfully
[2026-01-29 10:10:20] Reading authentication module...
[2026-01-29 10:10:25] Running tests...
[2026-01-29 10:10:30] FAILED tests/test_auth.py::test_login - AssertionError
[2026-01-29 10:10:35] Analyzing failure...
```

---

### 4.5 Cancel Task

Cancel a running or pending task.

```bash
sandbox task cancel <task-id> [options]
```

**Options:**
- `--reason <reason>`: Cancellation reason
- `--force`: Force immediate termination
- `--yes, -y`: Skip confirmation prompt

**Examples:**

```bash
# Cancel task (with confirmation)
sandbox task cancel task-001

# Cancel with reason
sandbox task cancel task-001 --reason "Duplicate task submitted"

# Force cancel without confirmation
sandbox task cancel task-001 --force --yes
```

**Output:**

```
Task task-001 is currently running.
Cancel this task? [y/N]: y

Task cancelled successfully
  ID: task-001
  Previous state: running
  New state: cancelled
  Cancelled at: 2026-01-29 10:30:00 UTC
```

---

### 4.6 List Artifacts

List artifacts collected from a task.

```bash
sandbox task artifacts <task-id> [options]
```

**Options:**
- `--format, -f <format>`: Output format: text, json, table (default: table)

**Examples:**

```bash
# List artifacts
sandbox task artifacts task-001

# List with JSON output
sandbox task artifacts task-001 --format json
```

**Output (table):**

```
NAME                    SIZE      TYPE              CREATED
fix-auth-bug.patch     4.5 KB    text/x-patch      2026-01-29 10:45:00
REPORT.md              2.3 KB    text/markdown     2026-01-29 10:45:00
test-results.xml      12.1 KB    application/xml   2026-01-29 10:45:00

Total: 3 artifacts, 18.9 KB
```

---

### 4.7 Download Artifact

Download an artifact from a task.

```bash
sandbox task download <task-id> <artifact-name> [options]
```

**Options:**
- `--output, -o <path>`: Output file path (default: current directory)
- `--verify`: Verify checksum after download

**Examples:**

```bash
# Download artifact to current directory
sandbox task download task-001 fix-auth-bug.patch

# Download to specific path
sandbox task download task-001 fix-auth-bug.patch --output /tmp/fix.patch

# Download and verify checksum
sandbox task download task-001 fix-auth-bug.patch --verify
```

**Output:**

```
Downloading fix-auth-bug.patch...
Downloaded 4.5 KB to fix-auth-bug.patch
Checksum: sha256:abc123... (verified)
```

---

## 5. Error Codes

Standard HTTP error codes and custom error types.

| HTTP Code | Error Type | Description |
|-----------|------------|-------------|
| 400 | `invalid_manifest` | Manifest validation failed |
| 400 | `invalid_parameter` | Invalid query parameter |
| 401 | `unauthorized` | Authentication required |
| 403 | `forbidden` | Insufficient permissions |
| 404 | `task_not_found` | Task does not exist |
| 404 | `artifact_not_found` | Artifact does not exist |
| 409 | `duplicate_task_id` | Task ID already exists |
| 409 | `task_already_terminal` | Task is in terminal state |
| 422 | `invalid_state_transition` | Invalid state transition |
| 429 | `rate_limit_exceeded` | Too many requests |
| 500 | `internal_error` | Server internal error |
| 503 | `service_unavailable` | Service temporarily unavailable |
| 507 | `insufficient_resources` | Not enough resources |

---

## 6. Authentication and Authorization

### 6.1 API Token Authentication

**Header:**
```
Authorization: Bearer <token>
```

**Example:**
```bash
curl -H "Authorization: Bearer sk-abc123..." \
  http://localhost:8122/api/v1/tasks
```

### 6.2 Task-Level Permissions

Future enhancement for multi-tenant environments:

- `task:create` - Submit new tasks
- `task:read` - View task status and logs
- `task:cancel` - Cancel tasks
- `task:artifacts` - Access artifacts

---

## 7. Rate Limits

Default rate limits per API key:

| Endpoint | Limit |
|----------|-------|
| `POST /api/v1/tasks` | 10 per minute |
| `GET /api/v1/tasks` | 60 per minute |
| `GET /api/v1/tasks/{id}` | 120 per minute |
| `GET /api/v1/tasks/{id}/logs` | 30 per minute |
| WebSocket connections | 10 concurrent |

Rate limit headers:
```
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 45
X-RateLimit-Reset: 1738152000
```

---

## 8. Versioning and Compatibility

### API Versioning

- Current version: `v1`
- Version specified in URL path: `/api/v1/...`
- Breaking changes will increment version number
- Legacy versions supported for 6 months after deprecation

### Manifest Versioning

- Current version: `1`
- Specified in manifest `version` field
- New manifest versions maintain backward compatibility where possible
- Deprecated fields trigger warnings but remain functional

---

## 9. Examples and Use Cases

### 9.1 Complete Task Submission Flow

```bash
# 1. Create manifest
cat > fix-bug.yaml <<EOF
version: "1"
kind: Task
metadata:
  id: ""
  name: "Fix authentication bug"
repository:
  url: "https://github.com/org/app.git"
  branch: "main"
claude:
  prompt: "Fix the bug in GitHub issue #42 and add tests"
lifecycle:
  artifact_patterns:
    - "*.patch"
EOF

# 2. Submit task
TASK_ID=$(sandbox task submit fix-bug.yaml --format json | jq -r '.task_id')
echo "Task ID: $TASK_ID"

# 3. Monitor progress
sandbox task status $TASK_ID --watch &

# 4. Follow logs
sandbox task logs $TASK_ID --follow

# 5. Download artifacts when complete
sandbox task artifacts $TASK_ID
sandbox task download $TASK_ID fix.patch
```

### 9.2 Programmatic Task Management (Python)

```python
import requests
import yaml

# Create task manifest
manifest = {
    "version": "1",
    "kind": "Task",
    "metadata": {
        "id": "",
        "name": "Automated refactoring"
    },
    "repository": {
        "url": "https://github.com/org/app.git",
        "branch": "main"
    },
    "claude": {
        "prompt": "Refactor authentication module"
    }
}

# Submit task
response = requests.post(
    "http://localhost:8122/api/v1/tasks",
    headers={"Content-Type": "application/json"},
    json=manifest
)
task_id = response.json()["task_id"]
print(f"Task submitted: {task_id}")

# Poll status
import time
while True:
    status = requests.get(f"http://localhost:8122/api/v1/tasks/{task_id}").json()
    print(f"State: {status['state']}")
    if status['state'] in ['completed', 'failed', 'cancelled']:
        break
    time.sleep(5)

# Download artifacts
artifacts = requests.get(f"http://localhost:8122/api/v1/tasks/{task_id}/artifacts").json()
for artifact in artifacts['artifacts']:
    print(f"Downloading {artifact['name']}...")
    data = requests.get(artifact['download_url']).content
    with open(artifact['name'], 'wb') as f:
        f.write(data)
```

---

## 10. Migration and Deployment

### 10.1 Manifest Templates

Common templates in `/etc/agentic-sandbox/templates/`:

- `bugfix.yaml` - Bug fix template
- `refactor.yaml` - Refactoring template
- `security-audit.yaml` - Security audit template
- `data-processing.yaml` - Data processing template

### 10.2 CI/CD Integration

**GitHub Actions Example:**

```yaml
name: Submit Agentic Task
on: [push]
jobs:
  submit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - name: Submit task
        run: |
          sandbox task submit .agentic/task.yaml \
            --server ${{ secrets.AGENTIC_SERVER }} \
            --wait
```

---

## Appendix A: Complete Schema Reference

See individual sections above for detailed schemas.

## Appendix B: Change Log

- 2026-01-29: Initial v1 specification
- Future: Rate limiting, authentication, multi-tenancy

---

**Document Metadata:**
- API Version: 1.0.0
- Document Version: 1.0.0
- Last Updated: 2026-01-29
- Authors: API Designer (Agentic Sandbox Team)
- Status: Draft

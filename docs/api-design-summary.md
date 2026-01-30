# Task Orchestration API - Design Summary

Version: 1.0.0
Date: 2026-01-29
Role: API Designer

## Executive Summary

This document summarizes the API design for the agentic-sandbox task orchestration system, covering interface contracts, data models, versioning strategy, and evolution paths.

---

## 1. Design Principles

### 1.1 Core Principles

1. **Simplicity First** - Minimal manifest should be trivial to write
2. **Declarative** - Users declare what they want, not how to do it
3. **Observable** - Rich real-time visibility into task execution
4. **Composable** - APIs work together naturally
5. **Stable** - Strong backward compatibility guarantees

### 1.2 API Style

- **REST** for CRUD operations (tasks, artifacts)
- **WebSocket** for real-time streaming (logs, events)
- **gRPC** for internal agent communication (existing)
- **CLI** for human-friendly interaction

---

## 2. Interface Contracts

### 2.1 Task Manifest (YAML/JSON)

**Purpose:** Declarative task definition
**Format:** YAML (primary), JSON (alternative)
**Versioning:** Explicit `version` field for evolution

**Key Design Decisions:**

1. **Minimal Valid Manifest**
   - Only 5 required fields: `version`, `kind`, `metadata.name`, `repository.url`, `repository.branch`, `claude.prompt`
   - All other fields have sensible defaults
   - Reduces barrier to entry

2. **Metadata-First Design**
   - Follows Kubernetes-style resource model
   - Familiar to DevOps/platform engineers
   - Enables future extensibility (CRDs, operators)

3. **Explicit Defaults**
   - Every optional field documents its default value
   - No "magic" behavior
   - Predictable across versions

4. **Network Isolation Modes**
   - Three levels: `isolated`, `outbound`, `full`
   - Default is `isolated` (secure by default)
   - `outbound` mode requires explicit allowlist

5. **Secret References, Not Values**
   - Secrets never embedded in manifest
   - References resolved at orchestration time
   - Supports multiple backends (env, vault, file)

**Example Minimal Manifest:**
```yaml
version: "1"
kind: Task
metadata:
  name: "Quick Test"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Run tests"
```

**Validation Rules:**
- Required fields enforced at submission
- Resource sizes validated (memory format, disk size)
- Git URLs validated (https:// or git@)
- Network mode vs allowed_hosts consistency

---

### 2.2 REST API

**Base URL:** `/api/v1/`

**Endpoint Design:**

| Method | Path | Purpose | Idempotent |
|--------|------|---------|------------|
| POST | `/tasks` | Submit task | No |
| GET | `/tasks` | List tasks | Yes |
| GET | `/tasks/{id}` | Get task | Yes |
| DELETE | `/tasks/{id}` | Cancel task | Yes |
| GET | `/tasks/{id}/logs` | Stream logs | Yes |
| GET | `/tasks/{id}/artifacts` | List artifacts | Yes |
| GET | `/tasks/{id}/artifacts/{name}` | Download artifact | Yes |

**Key Design Decisions:**

1. **RESTful Resource Modeling**
   - Tasks are first-class resources with unique IDs
   - Artifacts are sub-resources under tasks
   - Logs are a streaming sub-resource

2. **Idempotency**
   - All GET operations are idempotent
   - DELETE is idempotent (cancelling cancelled task is no-op)
   - POST for task submission is NOT idempotent (use unique IDs)

3. **Filtering and Pagination**
   - List endpoint supports rich filtering: `?state=running&label.team=platform`
   - Pagination via `limit` and `offset`
   - Total count always returned for UI pagination

4. **Content Negotiation**
   - Accept YAML or JSON for task submission
   - Return JSON for structured data
   - Return text/plain or NDJSON for logs

5. **Error Model**
   - Standard HTTP status codes
   - Custom error types in response body
   - Detailed error context where helpful

**Response Envelope:**
```json
{
  "task_id": "...",
  "state": "...",
  "message": "...",
  "links": {
    "self": "/api/v1/tasks/...",
    "logs": "/api/v1/tasks/.../logs"
  }
}
```

**Error Response:**
```json
{
  "error": "error_type",
  "message": "Human-readable message",
  "details": {
    "field": "...",
    "reason": "..."
  }
}
```

---

### 2.3 WebSocket Events

**Endpoint:** `ws://host:port/api/v1/tasks/stream`

**Purpose:** Real-time task updates without polling

**Event Types:**

1. **state_change** - Task state transitions
2. **output** - stdout/stderr output
3. **progress** - Execution progress updates
4. **metrics** - VM resource metrics
5. **error** - Error notifications

**Key Design Decisions:**

1. **Selective Subscription**
   - Filter by task_id: `?task_id=task-001`
   - Filter by event type: `?filter=state_change,output`
   - Subscribe to all tasks (admin view) or specific task (user view)

2. **Event Schema**
   - Every event has `type`, `task_id`, `timestamp`
   - Consistent JSON structure across event types
   - Self-describing events (no external schema required)

3. **Reliability**
   - Events are best-effort (fire and forget)
   - Clients should poll REST API for authoritative state
   - WebSocket is for "live updates", not "guaranteed delivery"

4. **Backpressure**
   - Server drops events if client is slow (bounded buffer)
   - Client receives "dropped events" notification
   - Prevents slow clients from blocking others

**Example Event:**
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

---

### 2.4 CLI Interface

**Command Structure:**
```
sandbox task <action> [args] [options]
```

**Actions:**
- `submit` - Submit task manifest
- `list` - List tasks
- `status` - Get task status
- `logs` - Stream logs
- `cancel` - Cancel task
- `artifacts` - List artifacts
- `download` - Download artifact

**Key Design Decisions:**

1. **Unified Command Namespace**
   - All task operations under `sandbox task`
   - Consistent with existing `sandbox vm`, `sandbox agents`
   - Clear hierarchy and discoverability

2. **Output Formats**
   - Default: human-friendly text
   - `--format json`: machine-parsable JSON
   - `--format table`: tabular output

3. **Interactive vs Non-Interactive**
   - Detect TTY for interactive prompts
   - `--yes` flag for CI/CD automation
   - Exit codes: 0=success, 1=error, 2=task failed

4. **Server Configuration**
   - `--server` flag or `AGENTIC_SERVER` env var
   - Supports local (`http://localhost:8122`) and remote servers
   - Config file at `~/.agentic-sandbox/config.yaml`

**Example Usage:**
```bash
# Human-friendly
sandbox task submit task.yaml
sandbox task logs task-001 --follow

# Automation-friendly
TASK_ID=$(sandbox task submit task.yaml --format json | jq -r '.task_id')
sandbox task status $TASK_ID --format json > status.json
```

---

## 3. Data Contracts

### 3.1 Task Lifecycle States

**State Machine:**
```
pending → staging → provisioning → ready → running → completing → completed
    ↓         ↓          ↓           ↓        ↓           ↓
cancelled  cancelled  cancelled   cancelled failed  failed/failed_preserved
```

**Terminal States:**
- `completed` - Success
- `failed` - Failure, VM destroyed
- `failed_preserved` - Failure, VM preserved for debugging
- `cancelled` - User cancelled

**Key Design Decisions:**

1. **Explicit State Transitions**
   - Each state has clear entry/exit conditions
   - Invalid transitions rejected with error
   - State history tracked in database (future)

2. **Failure Modes**
   - `failed` vs `failed_preserved` based on `lifecycle.failure_action`
   - Preserved VMs accessible via SSH for debugging
   - Automatic cleanup after 24h (configurable)

3. **Cancellation**
   - Graceful cancellation from any non-terminal state
   - `force` flag for immediate termination
   - VM cleanup follows `lifecycle.failure_action`

---

### 3.2 Task Progress Tracking

**Progress Model:**
```json
{
  "output_bytes": 45678,
  "tool_calls": 23,
  "current_tool": "Bash",
  "last_activity_at": "2026-01-29T10:25:30Z"
}
```

**Key Metrics:**
- **output_bytes** - Total output size (indicates activity)
- **tool_calls** - Number of Claude tool invocations
- **current_tool** - Currently executing tool (or null if idle)
- **last_activity_at** - Timestamp of last output (detect hangs)

**Use Cases:**
- Detect hung tasks (no activity for N minutes)
- Estimate completion based on tool call patterns
- UI progress indicators

---

### 3.3 Artifact Collection

**Artifact Model:**
```json
{
  "name": "report.md",
  "path": "reports/summary.md",
  "size_bytes": 2345,
  "content_type": "text/markdown",
  "checksum": "sha256:abc123...",
  "created_at": "2026-01-29T10:45:00Z",
  "download_url": "/api/v1/tasks/task-001/artifacts/report.md"
}
```

**Collection Process:**
1. Task specifies `lifecycle.artifact_patterns` (glob patterns)
2. On completion, orchestrator scans workspace for matches
3. Matched files copied to `/mnt/inbox/<task-id>/`
4. Checksum computed for integrity verification
5. Metadata stored in database

**Key Design Decisions:**

1. **Glob Patterns**
   - Standard Unix glob syntax (`**/*` for recursive)
   - Relative to workspace root
   - Multiple patterns OR'd together

2. **Storage Location**
   - Artifacts stored in virtiofs inbox (persistent)
   - Accessible from host at `~/inbox/<task-id>/`
   - Automatic cleanup after 7 days (configurable)

3. **Size Limits**
   - Max artifact size: 100MB per file (configurable)
   - Max total artifacts: 1GB per task (configurable)
   - Large files trigger warning, not failure

---

## 4. Versioning and Compatibility

### 4.1 API Versioning Strategy

**Approach:** URL-based versioning (`/api/v1/`, `/api/v2/`)

**Version Increments:**
- **Major (v1 → v2)**: Breaking changes (field removals, type changes)
- **Minor (v1.1)**: Additive changes (new fields, new endpoints)
- **Patch (v1.1.1)**: Bug fixes only

**Compatibility Policy:**
- New fields added with default values (backward compatible)
- Old fields deprecated before removal (6 month notice)
- Multiple versions supported simultaneously (N and N-1)

**Current Version:** v1
**Stable Since:** 2026-01-29

---

### 4.2 Manifest Versioning

**Version Field:** `version: "1"`

**Evolution Strategy:**
1. **Additive Changes** (version unchanged)
   - New optional fields with defaults
   - New enum values for existing fields
   - Example: Add `vm.gpu` field with default `false`

2. **Breaking Changes** (version increment)
   - Required field additions
   - Field type changes
   - Field removals
   - Example: Change `vm.memory` from string to integer

**Validation:**
- Server validates manifest version before processing
- Unsupported versions rejected with clear error
- Future versions may support `version: "1.1"` for minor versions

---

### 4.3 Protocol Buffers (gRPC)

**Current:** Proto3 with `TaskDefinition`, `TaskStatus` messages

**Compatibility:**
- Proto3 guarantees forward/backward compatibility
- New fields added with reserved numbers
- Never remove or repurpose field numbers
- Enum values never removed (only deprecated)

**Sync with REST API:**
- Proto messages map 1:1 to JSON responses
- Generated TypeScript types for web UI
- CLI uses JSON internally, not proto

---

## 5. Performance and Scalability

### 5.1 Performance Targets

| Operation | Target Latency | Target Throughput |
|-----------|----------------|-------------------|
| Submit task | < 50ms | 100 tasks/sec |
| Get task status | < 10ms | 1000 req/sec |
| List tasks (100) | < 100ms | 500 req/sec |
| Stream logs | < 1s first chunk | 10MB/sec per stream |
| WebSocket event | < 10ms fanout | 10k events/sec |

### 5.2 Resource Limits

**Per-Task Limits:**
- Max manifest size: 1MB
- Max task timeout: 168h (7 days)
- Max artifact size: 100MB per file, 1GB total
- Max log retention: 7 days

**System Limits:**
- Max concurrent tasks: 100 (configurable)
- Max WebSocket connections: 1000
- Max tasks in database: 1M (with archival)

### 5.3 Caching Strategy

**What to Cache:**
- Task list queries (60s TTL)
- Task status (5s TTL)
- Artifact metadata (no TTL, immutable)

**What NOT to Cache:**
- Task submission (must be fresh)
- Logs (real-time data)
- WebSocket events (real-time)

---

## 6. Security and Authorization

### 6.1 Authentication

**Current:** None (trusted local network)

**Future (v2):**
- API key authentication (`Authorization: Bearer <key>`)
- JWT tokens for user identity
- mTLS for agent connections

### 6.2 Authorization

**Current:** All users can submit/view/cancel all tasks

**Future (v2):**
- Task ownership (user who submitted)
- RBAC: viewer, submitter, admin roles
- Label-based policies (team isolation)

### 6.3 Secret Handling

**Design:**
- Secrets NEVER in manifest (only references)
- Secrets resolved at orchestration time on host
- Secrets injected into VM as environment variables
- Secrets never logged or returned in API responses

**Secret Sources:**
1. **env** - Host environment variable
2. **vault** - HashiCorp Vault (future)
3. **file** - File on host filesystem

---

## 7. Observability

### 7.1 Logging

**Structured Logging:**
- All API requests logged with trace ID
- Task state transitions logged
- Errors logged with full context

**Log Levels:**
- DEBUG: Request/response bodies
- INFO: State changes, completions
- WARN: Retries, slow operations
- ERROR: Failures, exceptions

### 7.2 Metrics (Prometheus)

**Endpoint:** `/metrics`

**Key Metrics:**
- `agentic_tasks_total{state}` - Counter of tasks by state
- `agentic_task_duration_seconds{state}` - Histogram of task durations
- `agentic_api_requests_total{endpoint,method,status}` - Counter of API requests
- `agentic_api_request_duration_seconds{endpoint}` - Histogram of API latency
- `agentic_websocket_connections` - Gauge of active WebSocket connections

### 7.3 Tracing

**Correlation IDs:**
- Every API request gets unique trace ID (UUIDv7)
- Trace ID propagated through all operations
- Logged in all messages for request correlation

**Future:**
- OpenTelemetry spans for distributed tracing
- Integration with Jaeger/Zipkin

---

## 8. Error Handling

### 8.1 Error Categories

1. **Client Errors (4xx)**
   - Invalid manifest
   - Invalid parameters
   - Resource not found
   - State conflicts

2. **Server Errors (5xx)**
   - Internal errors
   - Resource exhaustion
   - Service unavailable

### 8.2 Error Response Format

**Standard Structure:**
```json
{
  "error": "error_type",
  "message": "Human-readable description",
  "details": {
    "field": "vm.memory",
    "validation": "Must end with G or M"
  },
  "trace_id": "01HQXYZ..."
}
```

**Error Types:**
- `invalid_manifest` - Manifest validation failure
- `invalid_parameter` - Invalid query/path parameter
- `task_not_found` - Task doesn't exist
- `duplicate_task_id` - Task ID collision
- `task_already_terminal` - Cannot operate on terminal task
- `insufficient_resources` - Not enough CPU/memory/disk
- `internal_error` - Server error (log trace_id)

### 8.3 Retry Guidance

**Retryable (with backoff):**
- 503 Service Unavailable
- 429 Rate Limit Exceeded
- 500 Internal Server Error (transient)

**Not Retryable:**
- 400 Bad Request (fix input)
- 404 Not Found (check resource exists)
- 409 Conflict (resolve conflict)

---

## 9. Testing Strategy

### 9.1 Contract Testing

**API Contract Tests:**
- OpenAPI spec generated from code
- Contract tests verify spec compliance
- Breaking changes detected automatically

**Manifest Validation Tests:**
- Valid manifests accepted
- Invalid manifests rejected with clear errors
- Edge cases (empty strings, max sizes)

### 9.2 Integration Testing

**Scenarios:**
1. Complete task lifecycle (pending → completed)
2. Task cancellation at each state
3. Task failure and retry
4. Artifact collection and download
5. WebSocket event delivery
6. Concurrent task execution

### 9.3 Performance Testing

**Load Tests:**
- 100 concurrent task submissions
- 1000 status queries per second
- 100 log streams simultaneously
- 1000 WebSocket connections

---

## 10. Future Evolution

### 10.1 Planned Features (v1.x)

**Short Term (Q1 2026):**
- Task dependencies (DAG execution)
- Task templates (reusable manifests)
- Webhook notifications (task completion)
- Enhanced filtering (date ranges, full-text search)

**Medium Term (Q2 2026):**
- Task scheduling (cron-like)
- Task quotas per user/team
- Artifact expiration policies
- Log streaming to external systems

### 10.2 Breaking Changes (v2.0)

**Considered for v2:**
- Authentication required (API keys)
- Authorization policies (RBAC)
- Multi-tenancy (namespace isolation)
- Manifest schema changes (flatten structure)

### 10.3 Deprecation Process

**Steps:**
1. Announce deprecation (release notes, warnings)
2. Provide migration guide
3. Support both old and new for 6 months
4. Remove old API in major version bump

---

## 11. Documentation and Support

### 11.1 Documentation Structure

- **API Reference** - Complete endpoint documentation (this doc)
- **User Guide** - Task manifest guide with examples
- **CLI Reference** - Command-line usage
- **Integration Guide** - CI/CD integration examples
- **Troubleshooting** - Common issues and solutions

### 11.2 Example Library

**Common Scenarios:**
- Bug fix task with GitHub PR creation
- Security audit task with report generation
- Refactoring task with code quality checks
- Data processing task with artifact collection

### 11.3 SDK and Libraries

**Future:**
- Python SDK (`agentic-sdk-python`)
- TypeScript SDK (`@agentic/sdk-ts`)
- Go SDK (`github.com/agentic/sdk-go`)

---

## 12. Summary

This API design provides a **simple, stable, and observable** interface for task orchestration in the agentic-sandbox system.

**Key Strengths:**
1. **Low barrier to entry** - 5-line YAML for minimal task
2. **Rich observability** - REST + WebSocket for complete visibility
3. **Strong contracts** - Explicit versioning and compatibility guarantees
4. **Extensible** - Designed for evolution without breaking changes
5. **Well-tested** - Comprehensive test strategy across all layers

**Next Steps:**
1. Implement REST API endpoints in `management/src/http/tasks.rs`
2. Add WebSocket streaming in `management/src/ws/`
3. Implement CLI commands in `cli/src/commands/task.rs`
4. Write integration tests in `tests/e2e/test_task_api.py`
5. Generate OpenAPI spec from code

---

**Document Metadata:**
- Version: 1.0.0
- Date: 2026-01-29
- Author: API Designer
- Collaborators: System Analyst, Architecture Designer, Security Architect
- Status: Ready for Review

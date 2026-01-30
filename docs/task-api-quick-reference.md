# Task Orchestration API - Quick Reference

One-page reference for common operations.

---

## Minimal Task Manifest

```yaml
version: "1"
kind: Task
metadata:
  name: "My Task"
repository:
  url: "https://github.com/org/repo.git"
  branch: "main"
claude:
  prompt: "Your instructions here"
```

---

## REST API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/tasks` | Submit task |
| GET | `/api/v1/tasks` | List tasks |
| GET | `/api/v1/tasks/{id}` | Get task status |
| DELETE | `/api/v1/tasks/{id}` | Cancel task |
| GET | `/api/v1/tasks/{id}/logs` | Stream logs |
| GET | `/api/v1/tasks/{id}/artifacts` | List artifacts |
| GET | `/api/v1/tasks/{id}/artifacts/{name}` | Download artifact |

---

## CLI Commands

```bash
# Submit task
sandbox task submit task.yaml

# List tasks
sandbox task list
sandbox task list --state running
sandbox task list --label team=platform

# Get status
sandbox task status <task-id>
sandbox task status <task-id> --watch

# Stream logs
sandbox task logs <task-id> --follow

# Cancel task
sandbox task cancel <task-id>

# List artifacts
sandbox task artifacts <task-id>

# Download artifact
sandbox task download <task-id> <artifact-name>
```

---

## Task States

```
pending → staging → provisioning → ready → running → completing → completed
    ↓         ↓          ↓           ↓        ↓           ↓
cancelled  cancelled  cancelled   cancelled failed   failed/failed_preserved
```

**Terminal States:** `completed`, `failed`, `failed_preserved`, `cancelled`

---

## Common curl Examples

### Submit Task

```bash
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/yaml" \
  --data-binary @task.yaml
```

### List Tasks

```bash
# All tasks
curl http://localhost:8122/api/v1/tasks

# Running tasks
curl http://localhost:8122/api/v1/tasks?state=running

# With pagination
curl http://localhost:8122/api/v1/tasks?limit=50&offset=0
```

### Get Task Status

```bash
curl http://localhost:8122/api/v1/tasks/task-001
```

### Cancel Task

```bash
curl -X DELETE http://localhost:8122/api/v1/tasks/task-001
```

### Stream Logs

```bash
# All logs
curl http://localhost:8122/api/v1/tasks/task-001/logs

# Follow logs (requires SSE client)
curl -N http://localhost:8122/api/v1/tasks/task-001/logs?follow=true

# Last 100 lines
curl http://localhost:8122/api/v1/tasks/task-001/logs?tail=100
```

### List Artifacts

```bash
curl http://localhost:8122/api/v1/tasks/task-001/artifacts
```

### Download Artifact

```bash
curl http://localhost:8122/api/v1/tasks/task-001/artifacts/report.md \
  -o report.md
```

---

## WebSocket Connection

```javascript
const ws = new WebSocket('ws://localhost:8121/api/v1/tasks/stream?task_id=task-001');

ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  console.log('Event:', data.type, data);
};
```

**Event Types:** `state_change`, `output`, `progress`, `metrics`, `error`

---

## Python Example

```python
import requests
import yaml

# Submit task
with open('task.yaml') as f:
    manifest = yaml.safe_load(f)

response = requests.post(
    'http://localhost:8122/api/v1/tasks',
    json=manifest
)
task_id = response.json()['task_id']

# Wait for completion
import time
while True:
    status = requests.get(f'http://localhost:8122/api/v1/tasks/{task_id}').json()
    print(f"State: {status['state']}")
    if status['state'] in ['completed', 'failed', 'cancelled']:
        break
    time.sleep(5)

# Download artifacts
artifacts = requests.get(f'http://localhost:8122/api/v1/tasks/{task_id}/artifacts').json()
for artifact in artifacts['artifacts']:
    print(f"Downloading {artifact['name']}...")
    data = requests.get(f"http://localhost:8122{artifact['download_url']}").content
    with open(artifact['name'], 'wb') as f:
        f.write(data)
```

---

## Common Manifest Patterns

### Bug Fix with GitHub

```yaml
version: "1"
kind: Task
metadata:
  name: "Fix bug #42"
repository:
  url: "https://github.com/org/app.git"
  branch: "main"
claude:
  prompt: "Fix the bug in GitHub issue #42"
  mcp_config:
    mcpServers:
      github:
        command: "npx"
        args: ["-y", "@modelcontextprotocol/server-github"]
        env:
          GITHUB_TOKEN: "${GITHUB_TOKEN}"
vm:
  network_mode: "outbound"
  allowed_hosts: ["api.github.com"]
secrets:
  - name: "GITHUB_TOKEN"
    source: "env"
    key: "GITHUB_TOKEN"
lifecycle:
  artifact_patterns: ["*.patch"]
```

### Security Audit

```yaml
version: "1"
kind: Task
metadata:
  name: "Security Audit"
repository:
  url: "https://github.com/org/app.git"
  branch: "production"
claude:
  prompt: |
    Perform security audit:
    1. Run static analysis
    2. Check dependencies
    3. Generate report
  allowed_tools: ["Read", "Bash", "Grep", "Write"]
vm:
  network_mode: "isolated"  # No network for security
lifecycle:
  timeout: "8h"
  artifact_patterns: ["security-report.md"]
```

### Large VM Task

```yaml
version: "1"
kind: Task
metadata:
  name: "Heavy Processing"
repository:
  url: "https://github.com/org/app.git"
  branch: "main"
claude:
  prompt: "Process large dataset"
vm:
  cpus: 16
  memory: "32G"
  disk: "100G"
  profile: "agentic-dev"
lifecycle:
  timeout: "24h"
```

---

## Error Codes

| Code | Error Type | Description |
|------|------------|-------------|
| 400 | `invalid_manifest` | Manifest validation failed |
| 404 | `task_not_found` | Task doesn't exist |
| 409 | `duplicate_task_id` | Task ID already exists |
| 409 | `task_already_terminal` | Task is completed/failed |
| 507 | `insufficient_resources` | Not enough resources |

---

## Tips and Best Practices

1. **Always specify a unique `metadata.id`** to avoid duplicates
2. **Use `lifecycle.failure_action: preserve`** for debugging failed tasks
3. **Specify `lifecycle.artifact_patterns`** to collect outputs
4. **Use `network_mode: outbound`** with allowlist for internet access
5. **Reference secrets, don't embed them** in manifests
6. **Set reasonable timeouts** - default is 24h
7. **Use labels** for organization: `label.team=platform`
8. **Follow logs** to see real-time progress

---

## Health Checks

```bash
# Basic health
curl http://localhost:8122/api/health

# Detailed health
curl http://localhost:8122/api/v1/health

# Readiness probe
curl http://localhost:8122/api/v1/health/ready

# Liveness probe
curl http://localhost:8122/api/v1/health/live

# Prometheus metrics
curl http://localhost:8122/metrics
```

---

## Default Values

| Field | Default |
|-------|---------|
| `vm.profile` | `agentic-dev` |
| `vm.cpus` | `4` |
| `vm.memory` | `8G` |
| `vm.disk` | `40G` |
| `vm.network_mode` | `isolated` |
| `claude.headless` | `true` |
| `claude.skip_permissions` | `true` |
| `claude.output_format` | `stream-json` |
| `claude.model` | `claude-sonnet-4-5-20250929` |
| `lifecycle.timeout` | `24h` |
| `lifecycle.failure_action` | `destroy` |

---

**Quick Reference v1.0.0 | 2026-01-29**

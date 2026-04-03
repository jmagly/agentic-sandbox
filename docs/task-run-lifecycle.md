# Task/Run Lifecycle - Corrected Design

This document defines the complete lifecycle for agent task execution in agentic-sandbox, aligned with the actual design philosophy.

## Design Philosophy

**Key Principle:** These VMs exist to give AI agents *elevated access in a safer space*.

The security model is:
- **Inside VM:** Agent has full control (sudo, docker, filesystem)
- **Isolation:** KVM hardware virtualization protects the host
- **Network:** Outbound allowed, inbound restricted to management host
- **Secrets:** Ephemeral, injected at creation, rotated per VM

Docker containers are supported as a parallel runtime for faster iteration. In Docker mode:
- **Inside container:** Agent has full control within container limits
- **Isolation:** Container isolation (namespaces/cgroups) rather than hardware virtualization
- **Network:** Same modes (isolated, gateway, host) via runtime config
- **Secrets:** Injected via environment or mounted files

This is NOT a traditional hardened container. The agent SHOULD be able to:
- Install any software
- Modify system configuration
- Run Docker containers
- Access network resources
- Do whatever is needed to complete the task

## Lifecycle States

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              TASK LIFECYCLE                                  │
│                                                                             │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐    ┌──────────┐          │
│  │ PENDING  │───►│ STAGING  │───►│ PROVISIONING │───►│  READY   │          │
│  │          │    │          │    │              │    │          │          │
│  │ Queued   │    │ Clone    │    │ Create VM    │    │ Agent    │          │
│  │          │    │ repo     │    │ Inject       │    │ connected│          │
│  └──────────┘    │ Write    │    │ secrets      │    └────┬─────┘          │
│                  │ TASK.md  │    │ Start VM     │         │                │
│                  └──────────┘    └──────────────┘         │                │
│                                                           ▼                │
│  ┌──────────┐    ┌──────────┐    ┌──────────────┐    ┌──────────┐          │
│  │COMPLETED │◄───│COMPLETING│◄───│   RUNNING    │◄───│  START   │          │
│  │          │    │          │    │              │    │  TASK    │          │
│  │ Artifacts│    │ Collect  │    │ Claude Code  │    │          │          │
│  │ stored   │    │ artifacts│    │ executing    │    │ Execute  │          │
│  │ VM gone  │    │ Git diff │    │ streaming    │    │ claude   │          │
│  └──────────┘    └──────────┘    └──────────────┘    └──────────┘          │
│                                         │                                   │
│                        ┌────────────────┼────────────────┐                  │
│                        ▼                ▼                ▼                  │
│                  ┌──────────┐    ┌──────────────┐  ┌───────────┐           │
│                  │  FAILED  │    │   FAILED     │  │ CANCELLED │           │
│                  │          │    │  PRESERVED   │  │           │           │
│                  │ Cleanup  │    │              │  │ User      │           │
│                  │ VM gone  │    │ VM kept for  │  │ requested │           │
│                  │          │    │ debugging    │  │ stop      │           │
│                  └──────────┘    └──────────────┘  └───────────┘           │
└─────────────────────────────────────────────────────────────────────────────┘
```

## State Details

### PENDING
Task submitted, waiting in queue for resources.

**Entry:**
- Manifest validated
- Task ID assigned
- Secrets references validated (not resolved yet)

**Actions:**
- Wait for available VM slot
- Priority queue ordering

### STAGING
Prepare the workspace before VM creation.

**Entry:**
- Resources available
- Task dequeued

**Actions:**
1. Create task directory: `/srv/agentshare/tasks/{task_id}/`
2. Clone repository to `inbox/`
3. Write `TASK.md` with prompt and instructions
4. Initialize `outbox/progress/` files (stdout.log, stderr.log, events.jsonl)

**Storage Created:**
```
/srv/agentshare/tasks/{task_id}/
├── manifest.yaml          # Original submission
├── state.json             # Current state (for recovery)
├── inbox/                 # Cloned repo + TASK.md
│   ├── .git/
│   ├── {repo contents}
│   └── TASK.md            # Task instructions for Claude
└── outbox/
    ├── progress/
    │   ├── stdout.log     # Real-time stdout
    │   ├── stderr.log     # Real-time stderr
    │   └── events.jsonl   # Structured events
    └── artifacts/         # Collected at completion
```

### PROVISIONING
Create and start the runtime (VM or container).

**Entry:**
- Staging complete

**Actions:**
1. Generate ephemeral secret (256-bit)
2. Store SHA256 hash in `agent-hashes.json`
3. Generate ephemeral SSH keypair (VM runtime)
4. Allocate IP from pool (192.168.122.201-254) for VM runtime
5. Generate cloud-init (VM runtime) with:
   - Agent secret injected to `/etc/agentic-sandbox/agent.env`
   - SSH keys
   - MANAGEMENT_SERVER address
   - UFW rules (restrict inbound to management host)
6. Create qcow2 overlay from base image (VM runtime)
7. Define libvirt domain with virtiofs mounts (VM runtime):
   - `inbox` → `/mnt/inbox` (RW)
   - `outbox` → `/mnt/outbox` (RW)
8. For Docker runtime: create hardened container with bind mounts and runtime limits
   - `global` → `/mnt/global` (RO)
8. Start VM

**Cloud-Init Injects:**
```bash
# /etc/agentic-sandbox/agent.env
AGENT_ID=task-{task_id}
AGENT_SECRET={256-bit-hex}
MANAGEMENT_SERVER={management-host}:8120
```

### READY
VM running, agent connected to management server.

**Entry:**
- VM booted
- Cloud-init complete
- Agent client connected via gRPC

**Detection:**
- Agent sends Registration message with ID + secret
- Server validates SHA256(secret) against stored hash
- Registration acknowledged

**Agent Capabilities at READY:**
- Full sudo access
- Docker available
- All dev tools installed (agentic-dev profile)
- Can reach external network (outbound)
- virtiofs mounts available

### RUNNING
Claude Code executing the task.

**Entry:**
- READY confirmed
- Execute command dispatched

**Execution Command:**
```bash
claude --headless \
  --dangerously-skip-permissions \
  --output-format stream-json \
  --model {model} \
  --print "{prompt}" \
  2>&1 | tee /mnt/outbox/progress/stdout.log
```

**Real-Time Streaming:**
- stdout/stderr streamed via gRPC to management server
- Written to outbox/progress/ for persistence
- WebSocket broadcasts to dashboard clients
- Progress tracking: bytes, tool calls, current tool

**Agent Behaviors During RUNNING:**
- Full filesystem access in inbox
- Can install packages, run containers
- Can access network (git clone, npm install, etc.)
- Heartbeats every 30s with metrics

### COMPLETING
Task finished, collecting results.

**Entry:**
- Claude process exited (any exit code)

**Actions:**
1. Generate git diff: `git diff HEAD > {task_id}.patch`
2. List new files: `git ls-files --others --exclude-standard`
3. Collect files matching `lifecycle.artifact_patterns`
4. Copy to `outbox/artifacts/`
5. Write final metadata

**Artifacts Collected:**
```
outbox/artifacts/
├── {task_id}.patch           # All code changes
├── {task_id}-untracked.txt   # New files list
├── metadata.json             # Exit code, timing, stats
└── {pattern-matched files}   # User-specified patterns
```

### COMPLETED
Task finished successfully.

**Entry:**
- Artifact collection complete
- Exit code 0 (or configured success codes)

**Actions:**
1. Destroy VM via `virsh undefine --remove-all-storage`
2. Revoke ephemeral secrets
3. Remove SSH keys
4. Task directory retained for artifact access

### FAILED
Task failed, VM destroyed.

**Entry:**
- Any error during lifecycle
- Non-zero exit code + `failure_action: destroy`

**Actions:**
1. Save final state and error message
2. Collect any available artifacts
3. Destroy VM
4. Revoke secrets

### FAILED_PRESERVED
Task failed, VM kept for debugging.

**Entry:**
- Non-zero exit code + `failure_action: preserve`

**Actions:**
1. Save state and error
2. Keep VM running
3. Log SSH access info for debugging:
   ```
   SSH: ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/{vm} agent@{ip}
   ```

**User Actions:**
- SSH into VM to debug
- Manually destroy when done: `./scripts/destroy-vm.sh {vm}`

### CANCELLED
User-initiated cancellation.

**Entry:**
- User calls cancel endpoint
- From any non-terminal state

**Actions:**
1. Send SIGTERM to Claude process (if running)
2. Wait grace period (default 30s)
3. Send SIGKILL if needed
4. Collect any artifacts
5. Destroy VM

## Secrets Flow

```
                    ┌─────────────────────────────────────────┐
                    │           SECRET LIFECYCLE              │
                    └─────────────────────────────────────────┘

PROVISIONING                          RUNNING                    CLEANUP
     │                                   │                          │
     ▼                                   ▼                          ▼
┌─────────────┐                   ┌─────────────┐           ┌─────────────┐
│ Generate    │                   │ Agent reads │           │ Delete hash │
│ 256-bit     │                   │ agent.env   │           │ from JSON   │
│ secret      │                   │             │           │             │
└──────┬──────┘                   └──────┬──────┘           │ Delete SSH  │
       │                                 │                  │ keys        │
       ▼                                 ▼                  └─────────────┘
┌─────────────┐                   ┌─────────────┐
│ Store hash: │                   │ Send secret │
│ agent-      │                   │ in gRPC     │
│ hashes.json │                   │ metadata    │
└──────┬──────┘                   └──────┬──────┘
       │                                 │
       ▼                                 ▼
┌─────────────┐                   ┌─────────────┐
│ Inject      │                   │ Server      │
│ plaintext   │                   │ validates   │
│ via cloud-  │                   │ SHA256      │
│ init        │                   │ match       │
└─────────────┘                   └─────────────┘
```

## Distributed Deployment

VMs can run on remote hosts, connecting back to central management:

```bash
# On management host
./management/dev.sh    # Starts on 0.0.0.0:8120

# On worker host (remote)
./provision-vm.sh agent-remote-01 \
  --management 10.0.1.100:8120 \
  --start
```

**VM Configuration:**
- `MANAGEMENT_SERVER=10.0.1.100:8120` (remote address)
- `MANAGEMENT_HOST_IP=10.0.1.100` (for UFW rules)
- Agent connects outbound to management server
- UFW restricts inbound to management host IP

## Multi-Agent Patterns

### Parent-Child Orchestration
Agent spawns subtasks via management API:

```bash
# Inside running agent VM
curl -X POST http://${MANAGEMENT_SERVER}/api/v1/tasks \
  -H "Authorization: Bearer ${AGENT_SECRET}" \
  -d @subtask-manifest.yaml
```

### Shared Repository Access
Multiple agents on same repo use branch coordination:

```yaml
# Parent task
repository:
  url: https://github.com/org/repo
  branch: main

# Child tasks
repository:
  url: https://github.com/org/repo
  branch: agent-{task_id}  # Each agent gets own branch
```

### Result Aggregation
Parent collects child results from their outboxes:

```bash
# Parent can read child outboxes via global mount or API
curl http://${MANAGEMENT_SERVER}/api/v1/tasks/{child_id}/artifacts
```

## Failure Handling

### Retry Policy
| Stage | Max Retries | Backoff |
|-------|-------------|---------|
| Git clone | 3 | 5s, 10s, 20s |
| VM provision | 2 | 10s, 30s |
| Agent connect | 30 | 2s (5 min total) |

### Timeout Enforcement
| Timeout | Default | Config Key |
|---------|---------|------------|
| Stage timeout | 15 min | `lifecycle.stage_timeout` |
| Provision timeout | 10 min | `lifecycle.provision_timeout` |
| Task timeout | 24 hours | `lifecycle.timeout` |
| Hang detection | 30 min no output | `lifecycle.hang_timeout` |

### Checkpoint Recovery
State persisted after each transition:

```json
// /srv/agentshare/tasks/{id}/state.json
{
  "state": "running",
  "vm_name": "task-abc123",
  "vm_ip": "192.168.122.205",
  "started_at": "2025-01-29T10:00:00Z",
  "last_checkpoint": "2025-01-29T10:30:00Z"
}
```

On management server restart:
1. Scan task directories
2. Check VM status via libvirt
3. Reconnect to running agents
4. Resume monitoring

## Task Manifest Reference

```yaml
version: "1"
kind: Task

metadata:
  name: "Refactor authentication module"
  labels:
    team: platform
    priority: high

repository:
  url: https://github.com/org/repo
  branch: main
  # commit: abc123  # Optional: pin to commit
  # subpath: packages/auth  # Optional: subdirectory

claude:
  prompt: |
    Refactor the authentication module to use OAuth 2.0.
    Update all tests and documentation.
  model: claude-sonnet-4-5-20250929
  max_turns: 100
  # allowed_tools: [Read, Write, Edit, Bash, Glob, Grep]  # Optional whitelist

vm:
  profile: agentic-dev
  cpus: 4
  memory: 8G
  disk: 40G
  # network_mode: outbound  # isolated | outbound | full

secrets:
  - name: ANTHROPIC_API_KEY
    source: env
    key: ANTHROPIC_API_KEY
  - name: GITHUB_TOKEN
    source: env
    key: GITHUB_TOKEN

lifecycle:
  timeout: 24h
  failure_action: preserve  # destroy | preserve
  artifact_patterns:
    - "*.patch"
    - "coverage/**/*"
    - "reports/*.json"
```

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/tasks` | Submit task manifest |
| GET | `/api/v1/tasks` | List tasks (filter by state) |
| GET | `/api/v1/tasks/{id}` | Get task status |
| DELETE | `/api/v1/tasks/{id}` | Cancel task |
| GET | `/api/v1/tasks/{id}/logs` | Get stdout/stderr |
| GET | `/api/v1/tasks/{id}/artifacts` | List artifacts |
| GET | `/api/v1/tasks/{id}/artifacts/{name}` | Download artifact |
| WS | `/ws/tasks/{id}/stream` | Stream real-time output |

## Observability

### Metrics
```
agentic_tasks_total{state}
agentic_tasks_active{state}
agentic_task_duration_seconds{outcome}
agentic_vm_provision_duration_seconds
agentic_agent_connected
```

### Logs
Structured JSON logs with trace IDs:
```json
{
  "timestamp": "2025-01-29T10:30:00Z",
  "level": "info",
  "trace_id": "01945abc...",
  "task_id": "task-xyz",
  "message": "Task state transition",
  "from": "staging",
  "to": "provisioning"
}
```

### Dashboard
- Real-time terminal per agent (xterm.js)
- Metrics display (CPU, memory, disk)
- Task state timeline
- Artifact browser

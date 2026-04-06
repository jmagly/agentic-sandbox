# Operations Guide

Comprehensive day-to-day operational procedures for the agentic-sandbox runtime isolation platform.

**Audience:** Operators, SREs, DevOps engineers
**Last Updated:** 2026-02-07
**Status:** Production-ready

---

## Quick Reference

| Operation | Command |
|-----------|---------|
| **Start management server** | `cd management && ./dev.sh` |
| **Stop management server** | `cd management && ./dev.sh stop` |
| **View server logs** | `cd management && ./dev.sh logs` |
| **List agents** | `curl http://localhost:8122/api/v1/agents` |
| **Dashboard** | http://localhost:8122 |
| **Metrics** | http://localhost:8122/metrics |
| **Provision new VM** | `./images/qemu/provision-vm.sh agent-04 --profile agentic-dev --agentshare --start` |
| **Deploy agent update** | `./scripts/deploy-agent.sh agent-01` |
| **Restart VM** | `virsh shutdown agent-01 && virsh start agent-01` |
| **View agent logs** | `ssh agent@192.168.122.201 journalctl -u agentic-agent -f` |

---

## Table of Contents

1. [Daily Operations](#daily-operations)
2. [VM Management](#vm-management)
3. [Agent Deployment](#agent-deployment)
4. [Task Management](#task-management)
5. [Session Management](#session-management)
6. [Monitoring](#monitoring)
7. [Log Management](#log-management)
8. [Backup and Recovery](#backup-and-recovery)
9. [Maintenance Tasks](#maintenance-tasks)
10. [Troubleshooting](#troubleshooting)

---

## Daily Operations

### Starting the Management Server

#### Development Mode

```bash
cd management
./dev.sh              # Build if needed, start server
```

Server starts on:
- **gRPC**: `0.0.0.0:8120` (agent connections)
- **WebSocket**: `0.0.0.0:8121` (real-time streaming)
- **HTTP**: `0.0.0.0:8122` (dashboard and REST API)

#### Production Mode (systemd)

```bash
# Start
sudo systemctl start agentic-mgmt

# Enable auto-start on boot
sudo systemctl enable agentic-mgmt

# Check status
sudo systemctl status agentic-mgmt
```

#### Verify Server is Running

```bash
# Check HTTP endpoint
curl http://localhost:8122/api/v1/health

# Expected: {"status":"ok"}

# Check connected agents
curl http://localhost:8122/api/v1/agents | jq '.agents | length'

# Check metrics endpoint
curl http://localhost:8122/metrics | head -20
```

### Stopping the Management Server

#### Development Mode

```bash
cd management
./dev.sh stop
```

#### Production Mode

```bash
sudo systemctl stop agentic-mgmt
```

#### Force Kill (Emergency)

```bash
pkill -f agentic-mgmt

# Or with PID file
kill $(cat management/.run/mgmt.pid)
```

### Restarting the Management Server

#### Development Mode (with rebuild)

```bash
cd management
./dev.sh restart      # Stops, rebuilds, starts
```

#### Production Mode

```bash
sudo systemctl restart agentic-mgmt
```

**Important:** After restart, the server performs session reconciliation automatically. All agents will reconnect and report their active sessions. Orphaned sessions from the previous server instance will be cleaned up.

### Monitoring Agent Health

#### Dashboard View

Open http://localhost:8122 in browser to see:
- Connected agents and status (ready, busy, disconnected)
- CPU, memory, disk usage per agent
- Active sessions and tasks
- Live terminal thumbnails

#### API Query

```bash
# List all agents with details
curl http://localhost:8122/api/v1/agents | jq '.'

# Count agents by status
curl http://localhost:8122/api/v1/agents | jq '.agents | group_by(.status) | map({status: .[0].status, count: length})'

# Get specific agent details
curl http://localhost:8122/api/v1/agents | jq '.agents[] | select(.id=="agent-01")'
```

#### Prometheus Metrics

```bash
# Connected agents
curl -s http://localhost:8122/metrics | grep agentic_agents_connected

# Agent status distribution
curl -s http://localhost:8122/metrics | grep agentic_agents_by_status
```

### Checking Task Status

```bash
# List all tasks
curl http://localhost:8122/api/v1/tasks | jq '.tasks[] | {id, state, agent_id}'

# Count tasks by state
curl http://localhost:8122/api/v1/tasks | jq '.tasks | group_by(.state) | map({state: .[0].state, count: length})'

# Get specific task
curl http://localhost:8122/api/v1/tasks/{task-id} | jq '.'

# Stream task logs (Server-Sent Events)
curl -N http://localhost:8122/api/v1/tasks/{task-id}/logs
```

### Log Locations and Rotation

#### Management Server Logs

| Mode | Location | Rotation |
|------|----------|----------|
| Development | `management/.run/mgmt.log` | None (append-only) |
| Production (systemd) | `journalctl -u agentic-mgmt` | systemd journal (30 days default) |
| Production (file) | `LOG_FILE` env var path | Daily if `LOG_FILE_ROTATION=daily` |

#### Agent Logs (on VM)

```bash
# View live logs
ssh agent@192.168.122.201 journalctl -u agentic-agent -f

# View last 100 lines
ssh agent@192.168.122.201 journalctl -u agentic-agent -n 100

# View logs since boot
ssh agent@192.168.122.201 journalctl -u agentic-agent -b
```

#### Task Run Logs (agentshare)

Task logs are stored in the agent's inbox:

```bash
# On host
ls /srv/agentshare/inbox/agent-01/runs/

# Inside VM
ls ~/inbox/runs/

# View specific run
cat ~/inbox/runs/2026-02-07-143022-abc123/stdout.log
cat ~/inbox/runs/2026-02-07-143022-abc123/stderr.log
```

---

## VM Management

### Provisioning New VMs

#### Basic Provisioning

```bash
# Provision with agentic-dev profile (recommended)
./images/qemu/provision-vm.sh agent-04 \
  --profile agentic-dev \
  --agentshare \
  --start

# Provision with custom resources
./images/qemu/provision-vm.sh agent-05 \
  --profile agentic-dev \
  --agentshare \
  --cpus 8 \
  --memory 16384 \
  --disk 100G \
  --start
```

#### Provisioning Profiles

| Profile | Use Case | Tools Included |
|---------|----------|----------------|
| `agentic-dev` | Full development environment | Python, Node.js, Go, Rust, Docker, Claude Code, AI tools, modern CLI utils |
| `basic` | Minimal environment | SSH access only, basic utilities |

#### Post-Provision Checklist

```bash
# Verify VM is running
virsh list --all | grep agent-04

# Check VM can reach network
virsh domifaddr agent-04

# Wait for cloud-init to complete (may take 2-5 minutes)
ssh agent@192.168.122.204 cloud-init status --wait

# Verify agent service is running
ssh agent@192.168.122.204 systemctl status agentic-agent
```

### Starting VMs

```bash
# Start a single VM
virsh start agent-01

# Start all agent VMs
for vm in $(virsh list --name --inactive | grep ^agent-); do
  virsh start "$vm"
done

# Verify VM is running
virsh domstate agent-01
# Expected: running
```

### Stopping VMs

#### Graceful Shutdown

```bash
# Request clean shutdown (gives VM time to save state)
virsh shutdown agent-01

# Wait for shutdown to complete
virsh domstate agent-01
# Expected: shut off

# Shutdown all agent VMs
for vm in $(virsh list --name | grep ^agent-); do
  virsh shutdown "$vm"
done
```

#### Force Destroy (Emergency)

```bash
# Immediately terminate VM (like pulling power plug)
virsh destroy agent-01

# Use only when shutdown fails or hangs
```

### Destroying VMs (Complete Removal)

```bash
# Use the destroy-vm script (handles cleanup)
./scripts/destroy-vm.sh agent-01

# Manual destruction (if script unavailable)
virsh destroy agent-01       # Stop if running
virsh undefine agent-01      # Remove definition
sudo rm -rf /var/lib/libvirt/images/agent-01.qcow2  # Delete disk
```

**Warning:** This permanently deletes the VM and all data. Back up any important files from `~/inbox` first.

### Reprovisioning VMs (Rebuild in Place)

Rebuild a VM while preserving its IP address and agent ID:

```bash
# Reprovision with same profile
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev

# The script will:
# 1. Destroy the existing VM
# 2. Provision a new VM with the same name
# 3. Generate a new secret and update hashes
# 4. Preserve IP address assignment
```

**Use cases:**
- Upgrade base image
- Fix corrupted VM state
- Apply new provisioning profile
- Reset to clean state

### VM Network Information

VMs are assigned IPs in the `192.168.122.0/24` range:

| VM Name | Default IP | Calculation |
|---------|------------|-------------|
| agent-01 | 192.168.122.201 | 200 + VM number |
| agent-02 | 192.168.122.202 | 200 + VM number |
| agent-03 | 192.168.122.203 | 200 + VM number |

```bash
# Get IP from libvirt
virsh domifaddr agent-01

# Connect via SSH
ssh agent@192.168.122.201

# SSH key location
~/.ssh/agentic_ed25519
```

### VM Resource Monitoring

```bash
# View VM CPU and memory usage
virsh domstats agent-01

# List all VMs with resource allocation
virsh list --all

# View detailed info
virsh dominfo agent-01
```

---

## Agent Deployment

### Building the Agent Binary

```bash
cd agent-rs
cargo build --release

# Binary location: agent-rs/target/release/agent-client
# Verify build
ls -lh agent-rs/target/release/agent-client
```

### Deploying Agent to a Single VM

#### Standard Deployment

```bash
# Deploy to specific VM (builds if needed)
./scripts/deploy-agent.sh agent-01

# Force rebuild before deploy
./scripts/deploy-agent.sh agent-01 --rebuild

# Deploy with debug logging
./scripts/deploy-agent.sh agent-01 --debug
```

#### What the Script Does

1. Builds agent binary if needed
2. Retrieves plaintext secret from VM (`/etc/agentic-sandbox/agent.env`)
3. Copies binary to VM via SCP
4. Creates systemd service file
5. Enables and starts service
6. Verifies deployment

### Deploying Agent to All Running VMs

```bash
# Full development cycle: rebuild server + agent, deploy to all VMs
./scripts/dev-deploy-all.sh

# With debug logging on all agents
./scripts/dev-deploy-all.sh --debug
```

**Workflow:**
1. Rebuilds management server (`./dev.sh restart`)
2. Rebuilds agent binary (`cargo build --release`)
3. Deploys to all running agent VMs
4. Verifies connectivity

### Verifying Agent Connectivity

```bash
# Check agent status on VM
ssh agent@192.168.122.201 systemctl status agentic-agent

# View recent agent logs
ssh agent@192.168.122.201 journalctl -u agentic-agent -n 50

# Check if agent is connected to server
curl http://localhost:8122/api/v1/agents | jq '.agents[] | select(.id=="agent-01")'

# Expected: agent object with status "ready" or "busy"
```

### Debug Mode Logging

Debug mode enables verbose logging for troubleshooting:

```bash
# Deploy with debug mode
./scripts/deploy-agent.sh agent-01 --debug

# View debug logs
ssh agent@192.168.122.201 journalctl -u agentic-agent -f

# Debug logs include:
# - gRPC message details
# - Command execution traces
# - Heartbeat timing
# - Connection lifecycle events
```

To disable debug mode, redeploy without `--debug` flag.

### Secret Management

Agent secrets are ephemeral and generated during VM provisioning:

| Location | Content | Owner | Mode |
|----------|---------|-------|------|
| VM: `/etc/agentic-sandbox/agent.env` | Plaintext secret (`AGENT_SECRET=...`) | root | 600 |
| Host: `/var/lib/agentic-sandbox/secrets/agent-hashes.json` | SHA256 hashes | root | 644 |

**Important:**
- The deploy script reads the plaintext secret from the VM (requires SSH access)
- The management server validates using the hash stored on the host
- Secrets are rotated when a VM is reprovisioned

```bash
# View secret on VM (requires SSH)
ssh agent@192.168.122.201 sudo cat /etc/agentic-sandbox/agent.env

# View hashes on host
sudo cat /var/lib/agentic-sandbox/secrets/agent-hashes.json | jq '.'
```

### Troubleshooting Agent Deployment

| Issue | Cause | Solution |
|-------|-------|----------|
| "Invalid agent secret" | Using hash instead of plaintext | Use `deploy-agent.sh` (reads from VM) |
| Agent binary not found | Not built | `cd agent-rs && cargo build --release` |
| Service won't start | Wrong binary path | Check `ExecStart` in systemd unit |
| SSH connection refused | VM not ready | Wait for cloud-init to complete |
| Agent shows "disconnected" | Firewall blocking port 8120 | Check host firewall rules |

---

## Task Management

### Submitting Tasks via API

```bash
# Submit a task
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Create a Python script that processes CSV files",
    "repository": "https://github.com/user/repo",
    "model": "claude-sonnet-4-20250514",
    "timeout_seconds": 3600
  }'

# Response includes task_id
{"task_id": "task-abc123", "state": "pending"}
```

### Monitoring Task Progress

```bash
# Get task status
curl http://localhost:8122/api/v1/tasks/task-abc123 | jq '.'

# Sample response
{
  "id": "task-abc123",
  "state": "running",
  "agent_id": "agent-01",
  "created_at": "2026-02-07T14:30:00Z",
  "started_at": "2026-02-07T14:30:15Z",
  "timeout_seconds": 3600
}

# Stream live logs (SSE)
curl -N http://localhost:8122/api/v1/tasks/task-abc123/logs
```

### Task Lifecycle States

```
PENDING → STAGING → PROVISIONING → READY → RUNNING → COMPLETING → COMPLETED
                                                  ↘            ↘
                                                   FAILED   CANCELLED
```

| State | Description |
|-------|-------------|
| PENDING | Task queued, waiting for agent |
| STAGING | Preparing environment |
| PROVISIONING | Setting up VM and dependencies |
| READY | Ready to execute |
| RUNNING | Task executing |
| COMPLETING | Finalizing and collecting artifacts |
| COMPLETED | Successfully finished |
| FAILED | Execution failed |
| CANCELLED | Manually cancelled |

### Canceling Tasks

```bash
# Cancel a running task
curl -X DELETE http://localhost:8122/api/v1/tasks/task-abc123

# Verify cancellation
curl http://localhost:8122/api/v1/tasks/task-abc123 | jq '.state'
# Expected: "cancelled"
```

**Effects:**
- Task process terminated (SIGTERM then SIGKILL)
- Agent marked as available
- Task state updated to CANCELLED

### Retrieving Artifacts

Task artifacts are stored in the agent's inbox:

```bash
# List artifacts for a task
curl http://localhost:8122/api/v1/tasks/task-abc123/artifacts | jq '.'

# Sample response
{
  "artifacts": [
    {"path": "output.txt", "size": 1024},
    {"path": "results.json", "size": 5120}
  ]
}

# Retrieve artifact via agentshare
ls /srv/agentshare/inbox/agent-01/runs/{run-id}/

# Inside VM
ls ~/inbox/current/
```

---

## Session Management

Sessions are interactive terminals or background processes managed by the agent.

### Viewing Active Sessions

#### Dashboard View

Open http://localhost:8122 and navigate to the agent's panel to see:
- Active session names ("main", "claude", etc.)
- Session type (interactive, headless, background)
- Live terminal thumbnails for PTY sessions

#### API Query

```bash
# Get sessions for an agent
curl http://localhost:8122/api/v1/agents | jq '.agents[] | select(.id=="agent-01") | .sessions'

# Sample response
[
  {
    "command_id": "cmd-abc123",
    "session_name": "main",
    "type": "interactive",
    "started_at": "2026-02-07T14:30:00Z"
  }
]
```

### Killing Orphaned Sessions

Orphaned sessions are automatically cleaned up during reconnection, but you can manually kill them:

```bash
# SSH to agent VM
ssh agent@192.168.122.201

# List tmux sessions
tmux ls

# Kill specific session
tmux kill-session -t main

# Kill all tmux sessions
tmux kill-server
```

### Session Reconciliation After Restart

When the management server restarts, it performs session reconciliation automatically:

1. **Agent reconnects** to new server instance
2. **Server queries** agent for active sessions
3. **Server compares** reported sessions against known command IDs
4. **Server instructs** agent to kill unrecognized sessions
5. **Agent confirms** reconciliation complete

**Behavior:**
- **Valid sessions** (still in dispatcher) are preserved
- **Orphaned sessions** (from old server instance) are terminated
- **Brief network issues** do not kill sessions (graceful recovery)

**Monitoring:**

```bash
# Watch server logs during reconciliation
cd management && ./dev.sh logs | grep reconciliation

# Expected log sequence:
# "Received session report" (agent reports active sessions)
# "Session reconciliation complete" (cleanup finished)
```

**Metrics:**

```bash
# Session reconciliation counts
curl -s http://localhost:8122/metrics | grep agentic_session_reconciliations_total

# Sessions killed during reconciliation
curl -s http://localhost:8122/metrics | grep agentic_sessions_killed_total
```

---

## Monitoring

### Dashboard Access

Open http://localhost:8122 in browser for real-time monitoring:

**Key Features:**
- Agent status grid (connected, ready, busy, disconnected)
- CPU, memory, disk usage graphs per agent
- Active tasks and sessions
- Live terminal access
- WebSocket-based real-time updates

### Prometheus Metrics

Metrics endpoint: http://localhost:8122/metrics

#### Key Metrics to Watch

| Metric | Type | Description | Alert Threshold |
|--------|------|-------------|-----------------|
| `agentic_agents_connected` | Gauge | Total connected agents | Alert if < expected count |
| `agentic_agents_by_status{status="ready"}` | Gauge | Available agents | Warn if < 1 |
| `agentic_tasks_by_state{state="running"}` | Gauge | Active tasks | Info only |
| `agentic_commands_total` | Counter | Total commands executed | N/A |
| `agentic_commands_by_result{result="failed"}` | Counter | Failed commands | Warn if rate > 5% |
| `agentic_command_latency_seconds` | Histogram | Command execution time | Warn if P95 > 5s |
| `agentic_task_outcomes_total{outcome="success"}` | Counter | Successful tasks | N/A |
| `agentic_agentshare_inbox_bytes` | Gauge | Inbox storage usage | Warn if > 40GB (80% of 50GB quota) |

#### Example Prometheus Queries

```promql
# Agent availability SLO (target: 99%)
(avg_over_time(agentic_agents_by_status{status="ready"}[5m]) / scalar(agentic_agents_connected)) * 100

# Command failure rate (5-minute window)
(rate(agentic_commands_by_result{result="failed"}[5m]) / rate(agentic_commands_total[5m])) * 100

# Command latency P95
histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m]))

# Task success rate (1-hour window)
(rate(agentic_task_outcomes_total{outcome="success"}[1h]) /
 (rate(agentic_task_outcomes_total{outcome="success"}[1h]) +
  rate(agentic_task_outcomes_total{outcome="failure"}[1h]))) * 100
```

### Alert Thresholds

See `scripts/prometheus/rules/agentic-sandbox.yml` for complete alert definitions.

#### Critical Alerts (Page On-Call)

- **AgentDisconnected**: Agent unreachable for 2 minutes
- **ManagementServerDown**: Server unreachable for 2 minutes
- **AgentDiskFull**: Agent disk > 90% for 5 minutes
- **AgentInboxQuotaExceeded**: Inbox > 95% of 50GB quota
- **CommandExecutionStalled**: 0 commands executing with running tasks

#### Warning Alerts (Slack Notification)

- **AgentHighCPU**: CPU > 80% for 10 minutes
- **AgentHighMemory**: Memory > 85% for 10 minutes
- **HighCommandFailureRate**: Command failure > 5% for 10 minutes
- **HighTaskFailureRate**: Task failure > 10% for 30 minutes
- **SlowCommandExecution**: P95 latency > 5s for 10 minutes

### Grafana Dashboard

1. Access Grafana at http://localhost:3000 (default: admin/admin)
2. Import dashboard from `scripts/prometheus/agentic-sandbox.json`
3. View panels:
   - Fleet status (agent counts)
   - Agent availability over time
   - Command throughput and latency
   - Task state distribution
   - Error rates
   - Resource usage heatmap

### Setting Up Monitoring Stack

```bash
# Install Prometheus, Alertmanager, Grafana
sudo apt install -y prometheus prometheus-alertmanager grafana

# Deploy configurations
sudo cp scripts/prometheus/prometheus.yml /etc/prometheus/
sudo cp scripts/prometheus/rules/agentic-sandbox.yml /etc/prometheus/rules/
sudo cp scripts/prometheus/alertmanager.yml /etc/alertmanager/

# Restart services
sudo systemctl restart prometheus alertmanager grafana-server

# Verify targets
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | {job, health}'
```

See `docs/monitoring.md` for comprehensive monitoring setup guide.

---

## Log Management

### Server Logs

#### Development Mode

```bash
# Location
management/.run/mgmt.log

# Tail logs
cd management && ./dev.sh logs

# Search logs
grep ERROR management/.run/mgmt.log
```

#### Production Mode (systemd)

```bash
# View live logs
sudo journalctl -u agentic-mgmt -f

# Last 100 lines
sudo journalctl -u agentic-mgmt -n 100

# Logs since specific time
sudo journalctl -u agentic-mgmt --since "2026-02-07 14:00"

# Export to file
sudo journalctl -u agentic-mgmt --since today > server-logs.txt
```

#### Production Mode (file-based)

Configure via environment variables:

```bash
# In /etc/agentic-sandbox/management.env
LOG_FILE=/var/log/agentic-management/server.log
LOG_FILE_ROTATION=daily
LOG_FILE_RETENTION_DAYS=30
```

### Agent Logs

Located on each agent VM, managed by systemd:

```bash
# View live logs
ssh agent@192.168.122.201 journalctl -u agentic-agent -f

# Last 50 lines
ssh agent@192.168.122.201 journalctl -u agentic-agent -n 50

# Since last boot
ssh agent@192.168.122.201 journalctl -u agentic-agent -b

# Filter by priority
ssh agent@192.168.122.201 journalctl -u agentic-agent -p err
```

### Task Logs

Task execution logs are stored in the agent's inbox:

```bash
# On host (via agentshare)
ls /srv/agentshare/inbox/agent-01/runs/

# Inside VM
ls ~/inbox/runs/

# Each run has:
# - stdout.log (standard output)
# - stderr.log (standard error)
# - metadata.json (run information)

# View logs
cat ~/inbox/runs/2026-02-07-143022-abc123/stdout.log
tail -f ~/inbox/runs/current/stdout.log  # Current run
```

### Log Format Configuration

Set via `LOG_FORMAT` environment variable:

| Format | Use Case | Example |
|--------|----------|---------|
| `pretty` | Development (colored, human-readable) | `2026-02-07T14:30:45Z INFO agent connected agent_id="agent-01"` |
| `json` | Production (machine-readable, structured) | `{"timestamp":"2026-02-07T14:30:45Z","level":"INFO","message":"agent connected","agent_id":"agent-01"}` |
| `compact` | Minimal output (single-line) | `14:30:45 INFO agent connected agent_id="agent-01"` |

```bash
# Development (pretty, colored)
export LOG_FORMAT=pretty
export LOG_LEVEL=debug

# Production (JSON for aggregation)
export LOG_FORMAT=json
export LOG_LEVEL=info
export LOG_FILE=/var/log/agentic-management/server.log
```

### Log Retention

#### Systemd Journal

```bash
# Check current retention
journalctl --disk-usage

# Configure retention
sudo nano /etc/systemd/journald.conf
# Set: SystemMaxUse=1G, MaxRetentionSec=30day

# Restart journald
sudo systemctl restart systemd-journald
```

#### File-based Logs

Configured via `LOG_FILE_RETENTION_DAYS` environment variable (default: 7 days).

```bash
# Set retention in management.env
LOG_FILE_RETENTION_DAYS=30
```

Logs are rotated daily and old files deleted automatically.

### Centralized Log Aggregation (Optional)

For production deployments, consider shipping logs to a centralized system:

**Loki + Promtail:**

```bash
# Install Promtail on host
wget https://github.com/grafana/loki/releases/download/v2.9.0/promtail-linux-amd64.zip
unzip promtail-linux-amd64.zip
sudo mv promtail-linux-amd64 /usr/local/bin/promtail

# Configure to tail journal
sudo nano /etc/promtail/config.yml

# Point to Loki instance
sudo systemctl start promtail
```

**Elasticsearch + Filebeat:**

```bash
# Install Filebeat
curl -L -O https://artifacts.elastic.co/downloads/beats/filebeat/filebeat-8.11.0-amd64.deb
sudo dpkg -i filebeat-8.11.0-amd64.deb

# Configure inputs
sudo nano /etc/filebeat/filebeat.yml

# Point to Elasticsearch
sudo systemctl start filebeat
```

---

## Backup and Recovery

### What to Back Up

| Component | Location | Frequency | Retention |
|-----------|----------|-----------|-----------|
| **Agent secrets** | `/var/lib/agentic-sandbox/secrets/` | After each VM provision | Permanent |
| **VM disk images** | `/var/lib/libvirt/images/*.qcow2` | Weekly | 4 weeks |
| **Agentshare data** | `/srv/agentshare/inbox/` | Daily | 30 days |
| **Prometheus TSDB** | `/var/lib/prometheus/metrics2/` | Weekly | 90 days |
| **Configuration** | `/etc/agentic-sandbox/` | After changes | Permanent |
| **Management server binary** | `management/target/release/agentic-mgmt` | After build | N/A (rebuild from source) |

### Secret Management

Agent secrets are critical for agent-server authentication.

#### Backup Secrets

```bash
# Create backup directory
sudo mkdir -p /backup/agentic-sandbox/secrets

# Backup secret hashes
sudo cp /var/lib/agentic-sandbox/secrets/agent-hashes.json \
  /backup/agentic-sandbox/secrets/agent-hashes-$(date +%Y%m%d).json

# Verify backup
sudo cat /backup/agentic-sandbox/secrets/agent-hashes-*.json | jq '.'
```

#### Restore Secrets

```bash
# Restore from backup
sudo cp /backup/agentic-sandbox/secrets/agent-hashes-20260207.json \
  /var/lib/agentic-sandbox/secrets/agent-hashes.json

# Restart management server to reload secrets
cd management && ./dev.sh restart

# Agents will reconnect automatically if secrets match
```

**Important:** If restoring to a different host, you must also copy or regenerate the ephemeral SSH keys (`~/.ssh/agentic_ed25519*`).

### VM Image Backups

#### Create VM Snapshot

```bash
# Stop VM first for consistency
virsh shutdown agent-01

# Wait for shutdown
virsh domstate agent-01

# Create snapshot
sudo cp /var/lib/libvirt/images/agent-01.qcow2 \
  /backup/agentic-sandbox/vm-images/agent-01-$(date +%Y%m%d).qcow2

# Compress to save space
sudo gzip /backup/agentic-sandbox/vm-images/agent-01-*.qcow2

# Restart VM
virsh start agent-01
```

#### Live Backup (No Downtime)

```bash
# Create external snapshot
virsh snapshot-create-as agent-01 backup-$(date +%Y%m%d) \
  --disk-only --atomic

# Copy backing file
sudo cp /var/lib/libvirt/images/agent-01.qcow2 \
  /backup/agentic-sandbox/vm-images/

# Merge snapshot back
virsh blockcommit agent-01 vda --active --pivot
```

#### Restore VM from Backup

```bash
# Destroy existing VM
./scripts/destroy-vm.sh agent-01

# Restore disk image
sudo gunzip -c /backup/agentic-sandbox/vm-images/agent-01-20260207.qcow2.gz \
  > /var/lib/libvirt/images/agent-01.qcow2

# Recreate VM definition
virsh define /path/to/agent-01.xml

# Start VM
virsh start agent-01

# Verify agent reconnects
curl http://localhost:8122/api/v1/agents | jq '.agents[] | select(.id=="agent-01")'
```

### Agentshare Data Backups

```bash
# Backup all inbox data
sudo rsync -av --delete /srv/agentshare/inbox/ \
  /backup/agentic-sandbox/agentshare/inbox-$(date +%Y%m%d)/

# Backup specific agent's inbox
sudo rsync -av /srv/agentshare/inbox/agent-01/ \
  /backup/agentic-sandbox/agentshare/agent-01-$(date +%Y%m%d)/

# Restore inbox
sudo rsync -av /backup/agentic-sandbox/agentshare/agent-01-20260207/ \
  /srv/agentshare/inbox/agent-01/
```

### Disaster Recovery

#### Scenario: Host Failure

**Recovery Steps:**

1. **Install new host** with KVM/libvirt
2. **Restore secrets**:
   ```bash
   sudo mkdir -p /var/lib/agentic-sandbox/secrets
   sudo cp /backup/secrets/agent-hashes.json /var/lib/agentic-sandbox/secrets/
   ```
3. **Restore VM images**:
   ```bash
   sudo gunzip -c /backup/vm-images/*.qcow2.gz > /var/lib/libvirt/images/
   ```
4. **Restore VM definitions** (or re-provision VMs)
5. **Deploy management server**:
   ```bash
   cd management
   cargo build --release
   ./dev.sh start
   ```
6. **Start VMs**:
   ```bash
   virsh start agent-01
   virsh start agent-02
   ```
7. **Verify agents reconnect**:
   ```bash
   curl http://localhost:8122/api/v1/agents
   ```

#### Scenario: Management Server Corruption

**Recovery Steps:**

1. **Stop corrupted server**:
   ```bash
   cd management && ./dev.sh stop
   ```
2. **Rebuild from source**:
   ```bash
   cd management
   cargo clean
   cargo build --release
   ```
3. **Restore secrets** (if needed):
   ```bash
   sudo cp /backup/secrets/agent-hashes.json /var/lib/agentic-sandbox/secrets/
   ```
4. **Start server**:
   ```bash
   ./dev.sh start
   ```
5. **Agents automatically reconnect** and perform session reconciliation

#### Scenario: Agent VM Data Loss

**Recovery Steps:**

1. **Stop affected VM**:
   ```bash
   virsh destroy agent-01
   ```
2. **Restore inbox data** (if backed up):
   ```bash
   sudo rsync -av /backup/agentshare/agent-01-latest/ /srv/agentshare/inbox/agent-01/
   ```
3. **Restore VM image** (or reprovision):
   ```bash
   ./scripts/reprovision-vm.sh agent-01 --profile agentic-dev
   ```
4. **Deploy agent**:
   ```bash
   ./scripts/deploy-agent.sh agent-01
   ```
5. **Verify connectivity**:
   ```bash
   curl http://localhost:8122/api/v1/agents | jq '.agents[] | select(.id=="agent-01")'
   ```

---

## Maintenance Tasks

### Cleaning Up Old VMs

```bash
# List all VMs
virsh list --all

# Destroy and undefine unused VMs
for vm in agent-old-01 agent-old-02; do
  virsh destroy "$vm" 2>/dev/null || true
  virsh undefine "$vm"
  sudo rm -f /var/lib/libvirt/images/"$vm".qcow2
done

# Remove from secret hashes
sudo nano /var/lib/agentic-sandbox/secrets/agent-hashes.json
# Remove entries for deleted VMs, save

# Restart server to reload secrets
cd management && ./dev.sh restart
```

### Purging Completed Tasks

```bash
# List completed tasks older than 30 days
curl http://localhost:8122/api/v1/tasks | jq '.tasks[] | select(.state=="completed" and (.created_at | fromdateiso8601) < (now - 2592000))'

# Delete specific task
curl -X DELETE http://localhost:8122/api/v1/tasks/{task-id}

# Clean up inbox run logs older than 30 days
find /srv/agentshare/inbox/*/runs/ -type d -mtime +30 -exec rm -rf {} +
```

**Recommended:** Set up automated cleanup cron job:

```bash
# /etc/cron.daily/agentic-cleanup
#!/bin/bash
find /srv/agentshare/inbox/*/runs/ -type d -mtime +30 -delete
```

### Rotating Secrets

Secrets are rotated automatically when a VM is reprovisioned:

```bash
# Reprovision generates new secret
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev

# Or manually rotate:
# 1. Generate new secret
NEW_SECRET=$(openssl rand -hex 32)
NEW_HASH=$(echo -n "$NEW_SECRET" | sha256sum | cut -d' ' -f1)

# 2. Update hash on host
sudo jq --arg vm "agent-01" --arg hash "$NEW_HASH" \
  '.[$vm] = $hash' \
  /var/lib/agentic-sandbox/secrets/agent-hashes.json > /tmp/hashes.json
sudo mv /tmp/hashes.json /var/lib/agentic-sandbox/secrets/agent-hashes.json

# 3. Update secret on VM
ssh agent@192.168.122.201 "echo 'AGENT_SECRET=$NEW_SECRET' | sudo tee /etc/agentic-sandbox/agent.env"

# 4. Restart management server
cd management && ./dev.sh restart

# 5. Redeploy agent
./scripts/deploy-agent.sh agent-01
```

### Updating Base Images

When Ubuntu releases a new base image:

```bash
# Download new base image
cd images/qemu
wget https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img

# Backup old image
mv ubuntu-24.04-server-cloudimg-amd64.img ubuntu-24.04-server-cloudimg-amd64.img.old

# Rename new image
mv ubuntu-24.04-server-cloudimg-amd64.img ubuntu-24.04-server-cloudimg-amd64.img

# Reprovision VMs with new base image
for vm in agent-01 agent-02 agent-03; do
  ./scripts/reprovision-vm.sh "$vm" --profile agentic-dev
done
```

### Disk Space Management

```bash
# Check agentshare usage
du -sh /srv/agentshare/inbox/*

# Check VM disk usage
ssh agent@192.168.122.201 df -h

# Clean up Docker images on agent VMs
ssh agent@192.168.122.201 docker system prune -a -f

# Check host disk space
df -h /var/lib/libvirt/images

# Compress old VM images
sudo gzip /var/lib/libvirt/images/agent-old-*.qcow2
```

### Database Maintenance (If Applicable)

Currently, the system does not use a persistent database (state in memory). Future versions may add persistence.

### Log Rotation

Logs are rotated automatically, but you can manually trigger rotation:

```bash
# Systemd journal
sudo journalctl --rotate
sudo journalctl --vacuum-time=30d

# File-based logs (if configured)
sudo logrotate -f /etc/logrotate.d/agentic-management
```

---

## Troubleshooting

### Management Server Won't Start

**Symptom:** Server fails to start or exits immediately

**Check 1: Port already in use**

```bash
# Find process using port 8120
sudo ss -tlnp | grep 8120
# Or
sudo lsof -i :8120

# Kill process
sudo kill <pid>

# Or use dev.sh stop
cd management && ./dev.sh stop
```

**Check 2: Secrets directory missing**

```bash
# Verify secrets directory exists
ls -la /var/lib/agentic-sandbox/secrets/

# Create if missing
sudo mkdir -p /var/lib/agentic-sandbox/secrets
sudo chown $USER:$USER /var/lib/agentic-sandbox/secrets

# Or use dev mode secrets
export SECRETS_DIR="$PWD/.run/secrets"
mkdir -p .run/secrets
```

**Check 3: Check logs**

```bash
# Development mode
cat management/.run/mgmt.log

# Production mode
sudo journalctl -u agentic-mgmt -n 50
```

### Agent Not Connecting

**Symptom:** Agent shows as "disconnected" in dashboard

**Check 1: Agent service status**

```bash
# Check if service is running
ssh agent@192.168.122.201 systemctl status agentic-agent

# Expected: active (running)

# If failed, view logs
ssh agent@192.168.122.201 journalctl -u agentic-agent -n 50
```

**Check 2: Network connectivity**

```bash
# Ping host from VM
ssh agent@192.168.122.201 ping -c 3 192.168.122.1

# Test gRPC port
ssh agent@192.168.122.201 nc -zv 192.168.122.1 8120

# Expected: Connection succeeded
```

**Check 3: Secret validation**

```bash
# View secret on VM
ssh agent@192.168.122.201 sudo cat /etc/agentic-sandbox/agent.env

# View hash on host
sudo cat /var/lib/agentic-sandbox/secrets/agent-hashes.json | jq '.'

# Verify hash matches
SECRET=$(ssh agent@192.168.122.201 "sudo grep AGENT_SECRET /etc/agentic-sandbox/agent.env | cut -d= -f2")
EXPECTED_HASH=$(echo -n "$SECRET" | sha256sum | cut -d' ' -f1)
echo "Expected hash: $EXPECTED_HASH"
```

**Check 4: Firewall rules**

```bash
# Check host firewall
sudo ufw status
sudo iptables -L -n | grep 8120

# Check VM firewall
ssh agent@192.168.122.201 sudo ufw status
```

### High Memory Usage on Management Server

**Symptom:** Server consuming excessive memory

**Check 1: Number of agents**

```bash
# Each agent uses ~10KB idle
curl http://localhost:8122/api/v1/agents | jq '.agents | length'
```

**Check 2: Output buffering**

Output buffering can grow under heavy load. Restart server to clear buffers:

```bash
cd management && ./dev.sh restart
```

**Check 3: Log level**

Debug logging increases memory overhead. Switch to info level:

```bash
export RUST_LOG=info
cd management && ./dev.sh restart
```

### Task Stuck in PENDING State

**Symptom:** Task never progresses past PENDING

**Check 1: Agent availability**

```bash
# Check if any agents are ready
curl http://localhost:8122/api/v1/agents | jq '.agents[] | select(.status=="ready")'

# Expected: At least one agent with status "ready"
```

**Check 2: Task queue backlog**

```bash
# Count pending tasks
curl http://localhost:8122/api/v1/tasks | jq '[.tasks[] | select(.state=="pending")] | length'

# If > 10, may indicate backlog
```

**Check 3: Server logs**

```bash
# Look for dispatch errors
cd management && ./dev.sh logs | grep -i dispatch
```

### Session Shows Garbled Terminal Output

**Symptom:** Terminal displays corrupted or duplicated text

**Check 1: Session reconciliation**

This typically happens when the server restarted and an old session is still running.

```bash
# Trigger reconciliation manually by restarting agent
ssh agent@192.168.122.201 sudo systemctl restart agentic-agent

# Agent will reconnect and reconcile sessions
```

**Check 2: Kill orphaned tmux sessions**

```bash
# SSH to VM
ssh agent@192.168.122.201

# List tmux sessions
tmux ls

# Kill conflicting session
tmux kill-session -t main

# Open new session from dashboard
```

### Agentshare Quota Exceeded

**Symptom:** Agent cannot write to inbox

**Check 1: Check inbox usage**

```bash
# Check per-agent usage
du -sh /srv/agentshare/inbox/*

# Check total usage
df -h /srv/agentshare
```

**Check 2: Clean up old runs**

```bash
# Delete runs older than 7 days
find /srv/agentshare/inbox/agent-01/runs/ -type d -mtime +7 -exec rm -rf {} +

# Verify space freed
du -sh /srv/agentshare/inbox/agent-01
```

**Check 3: Increase quota (if needed)**

```bash
# Edit provisioning script quotas
nano images/qemu/provision-vm.sh
# Find: INBOX_QUOTA_GB=50
# Change to: INBOX_QUOTA_GB=100

# Reprovision VM with new quota
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev
```

### VM Won't Start After Host Reboot

**Symptom:** `virsh start agent-01` fails

**Check 1: Libvirt service running**

```bash
sudo systemctl status libvirtd

# If not running
sudo systemctl start libvirtd
```

**Check 2: Network bridge**

```bash
# Check virsh network
virsh net-list --all

# If "default" network is inactive
virsh net-start default
virsh net-autostart default
```

**Check 3: VM definition exists**

```bash
# List defined VMs
virsh list --all

# If VM missing, restore definition
virsh define /etc/libvirt/qemu/agent-01.xml

# Or reprovision
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev
```

### Prometheus Metrics Not Appearing

**Symptom:** Dashboard or Grafana shows no data

**Check 1: Metrics endpoint accessible**

```bash
# Test endpoint
curl http://localhost:8122/metrics | head -20

# Expected: Prometheus text format output
```

**Check 2: Prometheus scrape config**

```bash
# Verify config
sudo cat /etc/prometheus/prometheus.yml | grep -A 5 management-server

# Test config syntax
promtool check config /etc/prometheus/prometheus.yml
```

**Check 3: Prometheus targets**

```bash
# Check target health
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="management-server")'

# Expected: "health": "up"
```

**Check 4: Restart Prometheus**

```bash
sudo systemctl restart prometheus

# Verify restart
sudo systemctl status prometheus
```

---

## Quick Command Reference

### Daily Checks

```bash
# Server status
curl http://localhost:8122/api/v1/health

# Connected agents
curl http://localhost:8122/api/v1/agents | jq '.agents | length'

# Running tasks
curl http://localhost:8122/api/v1/tasks | jq '[.tasks[] | select(.state=="running")] | length'

# Disk usage
df -h /srv/agentshare

# View dashboard
xdg-open http://localhost:8122
```

### Weekly Maintenance

```bash
# Check for updates
cd ~/dev/agentic-sandbox && git pull

# Rebuild server
cd management && ./dev.sh restart

# Redeploy agents
./scripts/dev-deploy-all.sh

# Clean up old runs
find /srv/agentshare/inbox/*/runs/ -type d -mtime +30 -exec rm -rf {} +

# Backup secrets
sudo cp /var/lib/agentic-sandbox/secrets/agent-hashes.json \
  /backup/secrets/agent-hashes-$(date +%Y%m%d).json
```

### Emergency Procedures

```bash
# Kill all agent processes
for vm in $(virsh list --name | grep ^agent-); do
  virsh destroy "$vm"
done

# Stop management server
cd management && ./dev.sh stop
pkill -f agentic-mgmt

# Restart everything
cd management && ./dev.sh start
for vm in $(virsh list --name --inactive | grep ^agent-); do
  virsh start "$vm"
done
```

---

## Operating with AIWG Serve

If `AIWG_SERVE_ENDPOINT` is configured, the management server registers with [aiwg serve](https://github.com/jmagly/aiwg/blob/main/docs/serve-guide.md) and streams events to the AIWG operator dashboard. This section covers operational procedures specific to AIWG-connected deployments.

### Quick Reference

| Operation | Command |
|-----------|---------|
| **Check aiwg serve connection** | Look for `aiwg serve WS connected` in server logs |
| **Verify sandbox registered** | `curl http://<aiwg-serve>:7337/api/sandboxes` |
| **List pending HITL requests** | `curl http://localhost:8122/api/v1/hitl` |
| **Respond to HITL** | `curl -X POST http://localhost:8122/api/v1/hitl/{id}/respond -d '{"response":"y"}'` |
| **Check connection status** | `./dev.sh logs \| grep -i aiwg` |

### Verifying AIWG Integration Health

```bash
# Server logs should show:
# INFO Registered with aiwg serve at http://localhost:7337
# DEBUG aiwg serve WS connected: ws://localhost:7337/ws/sandbox/...

# Confirm sandbox appears in aiwg serve registry
curl http://<aiwg-serve>:7337/api/sandboxes | jq '.[] | {id: .sandbox_id, name: .name}'

# List agents as seen by aiwg serve
curl http://<aiwg-serve>:7337/api/sandboxes/<sandbox-id>/agents | jq
```

### Monitoring the Event Stream

Events are pushed fire-and-forget over a persistent WebSocket. Watch for disconnects in logs:

```bash
# Watch for reconnects — normal during aiwg serve restarts
./dev.sh logs | grep -E "aiwg serve (WS|connection|reconnect)"

# If you see repeated reconnect attempts, check aiwg serve is running
curl http://<aiwg-serve>:7337/api/health
```

### Operating HITL

When agents are running and prompt detection fires, HITL requests appear in both the local dashboard and the aiwg serve dashboard:

```bash
# See all pending HITL requests
curl http://localhost:8122/api/v1/hitl | jq

# Respond to a specific request (text is injected into PTY stdin)
curl -X POST http://localhost:8122/api/v1/hitl/{hitl_id}/respond \
  -H "Content-Type: application/json" \
  -d '{"response": "yes"}'

# Requests resolve automatically — the session dedup slot clears after response
```

### Temporary AIWG Disconnection

If you need to temporarily disable the AIWG integration without stopping the server:

```bash
# Remove or comment AIWG_SERVE_ENDPOINT from .run/dev.env then restart
./dev.sh restart

# The server will operate in standalone mode — all local features continue working
```

## Related Documentation

- [README.md](../README.md) - Project overview and quick start
- [CLAUDE.md](../CLAUDE.md) - Development guidance
- [monitoring.md](./monitoring.md) - Comprehensive monitoring setup
- [SESSION_RECONCILIATION.md](./SESSION_RECONCILIATION.md) - Session lifecycle details
- [LIFECYCLE.md](./LIFECYCLE.md) - VM and task lifecycle management
- [aiwg serve guide](https://github.com/jmagly/aiwg/blob/main/docs/serve-guide.md) - AIWG operator dashboard documentation

---

**Last Updated:** 2026-02-07
**Maintained By:** Platform Team
**Feedback:** Open an issue at https://git.integrolabs.net/roctinam/agentic-sandbox/issues

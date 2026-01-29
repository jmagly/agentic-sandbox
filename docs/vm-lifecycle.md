# VM/Agent Lifecycle Documentation

This document covers the complete lifecycle of agent VMs in the Agentic Sandbox system, from provisioning through operation to teardown.

## Table of Contents

- [System Overview](#system-overview)
- [Architecture](#architecture)
- [Lifecycle Scripts](#lifecycle-scripts)
- [Security Model](#security-model)
- [Agentshare Storage](#agentshare-storage)
- [Management Server](#management-server)
- [Secrets Management](#secrets-management)
- [Common Operations](#common-operations)
- [Troubleshooting](#troubleshooting)

## System Overview

The Agentic Sandbox provides isolated QEMU/KVM VMs for running AI agent processes. Each VM:

- Runs Ubuntu 24.04 with a service account (`agent`) that has sudo NOPASSWD
- Connects back to a management server via gRPC for command dispatch and output streaming
- Has virtiofs-mounted shared storage (global read-only, inbox read-write)
- Is provisioned from qcow2 overlay images for fast boot
- Uses ephemeral secrets rotated on each provisioning

## Architecture

```
Host (grissom)
├── Management Server (Rust)      # gRPC :8120, WS :8121, HTTP :8122
│   ├── Agent Registry            # DashMap of connected agents
│   ├── Command Dispatcher        # Send commands to agents
│   ├── Output Aggregator         # Collect stdout/stderr
│   └── WebSocket Hub             # Stream to dashboard
├── Agentshare (/srv/agentshare)
│   ├── global/                   # Read-only content (prompts, tools, configs)
│   ├── global-ro -> global       # Symlink for virtiofs RO enforcement
│   ├── staging/                  # Temp content before publishing
│   └── {vm}-inbox/               # Per-VM read-write inbox
└── VMs (libvirt/QEMU)
    └── agent-test-01
        ├── /mnt/global (RO)      # virtiofs: agentglobal
        ├── /mnt/inbox (RW)       # virtiofs: agentinbox
        ├── ~/global -> /mnt/global
        ├── ~/inbox -> /mnt/inbox
        └── agent-client service  # Rust gRPC agent
```

### Component Relationships

**Management Server** coordinates all agent VMs:
- Accepts gRPC connections from agents on port 8120
- Validates agent secrets against `agent-hashes.json`
- Dispatches commands and aggregates output
- Streams real-time updates to WebSocket clients on port 8121
- Serves dashboard UI on port 8122

**Agent VMs** run in isolated QEMU/KVM instances:
- Connect to management server at startup
- Execute commands via the agent service running as the `agent` user
- Stream stdout/stderr back to server
- Send heartbeat metrics every 30 seconds

**Agentshare** provides shared storage via virtiofs:
- Global content is read-only across all VMs
- Each VM has a dedicated read-write inbox
- Fast, low-latency access without network overhead

## Lifecycle Scripts

### 1. provision-vm.sh (Create VM)

**Location:** `images/qemu/provision-vm.sh`

Creates a new agent VM from a base image with complete initialization.

**What it does:**
- Creates qcow2 overlay disk (instant provisioning using backing file)
- Generates ephemeral secret (256-bit hex) and stores SHA256 hash in `agent-hashes.json` and `agent-tokens`
- Generates ephemeral ed25519 SSH key pair (private on host, public in cloud-init)
- Allocates static IP in 192.168.122.201-254 range
- Generates deterministic MAC address from VM name hash
- Creates cloud-init ISO with hostname, users, packages, agent.env, health server, mount config
- Defines libvirt domain with virtiofs filesystems
- Adds DHCP reservation for static IP

**Usage:**
```bash
sudo ./images/qemu/provision-vm.sh [OPTIONS] <vm-name>

Options:
  --cpus N             CPU count (default: 4)
  --memory SIZE        RAM (default: 8G)
  --disk SIZE          Disk (default: 40G)
  --agentshare         Enable virtiofs mounts (global RO, inbox RW)
  --start              Start VM immediately
  --wait               Wait for SSH ready (implies --start)
  --profile NAME       Cloud-init profile (basic, agentic-dev)
```

**Examples:**
```bash
# Create VM with defaults
sudo ./images/qemu/provision-vm.sh agent-01

# Create VM with agentshare and start immediately
sudo ./images/qemu/provision-vm.sh --agentshare --start --wait agent-02

# Create VM with custom resources
sudo ./images/qemu/provision-vm.sh --cpus 8 --memory 16G --disk 100G agent-03
```

**Output Files:**
- VM disk: `/var/lib/libvirt/images/agent-{name}/{name}.qcow2`
- Cloud-init ISO: `/var/lib/libvirt/images/agent-{name}/cloud-init.iso`
- SSH private key: `/var/lib/agentic-sandbox/secrets/ssh-keys/{name}`
- SSH public key: `/var/lib/agentic-sandbox/secrets/ssh-keys/{name}.pub`
- Agent secret hash: Added to `/var/lib/agentic-sandbox/secrets/agent-hashes.json`
- Agent token: Added to `/var/lib/agentic-sandbox/secrets/agent-tokens`

### 2. provision-vm-agent.sh (Deploy Agent Binary)

**Location:** `scripts/provision-vm-agent.sh`

Deploys the compiled agent binary into a provisioned VM via SSH/SCP.

**What it does:**
- Uses ephemeral SSH key from `secrets/ssh-keys/<vm-name>`
- Auto-detects VM IP from IP registry or libvirt
- Reads agent.env already on VM (does NOT generate new secrets)
- Copies binary via SCP to `/usr/local/bin/agent-client`
- Installs systemd unit with security hardening (NoNewPrivileges, ProtectSystem, etc.)
- Starts and verifies the service

**Usage:**
```bash
sudo ./scripts/provision-vm-agent.sh <vm-name> [OPTIONS]

Options:
  --ip ADDRESS         Override VM IP address
  --variant rust|python  Agent variant (default: rust)
  --server HOST:PORT   Override management server in agent.env
  --no-start           Install but don't start the service
  --force              Overwrite existing binary
```

**Examples:**
```bash
# Deploy agent to VM
sudo ./scripts/provision-vm-agent.sh agent-01

# Deploy with explicit IP
sudo ./scripts/provision-vm-agent.sh agent-01 --ip 192.168.122.201

# Deploy without starting service
sudo ./scripts/provision-vm-agent.sh agent-01 --no-start

# Force update existing agent
sudo ./scripts/provision-vm-agent.sh agent-01 --force
```

**Prerequisites:**
- VM must be provisioned and running
- SSH must be accessible (cloud-init complete)
- Agent binary must be built: `cd agent-rs && cargo build --release`

**Systemd Unit Security:**
The installed service runs with security hardening:
- `User=agent` - Runs as unprivileged user
- `NoNewPrivileges=true` - Cannot gain privileges
- `ProtectSystem=strict` - Read-only system directories
- `ProtectHome=read-only` - Read-only home directories
- `ReadWritePaths=/home/agent /tmp /mnt/inbox` - Explicit write access

### 3. destroy-vm.sh (Teardown)

**Location:** `scripts/destroy-vm.sh`

Clean teardown with data preservation options.

**What it does:**
- Archives non-empty inbox to `/srv/agentshare/archived/{vm}-inbox-{timestamp}/`
- Stops and undefines VM from libvirt
- Removes VM storage directory
- Removes DHCP reservation from virbr0
- Cleans up secrets (SSH keys, agent-tokens, agent-hashes.json)

**Usage:**
```bash
sudo ./scripts/destroy-vm.sh <vm-name> [OPTIONS]

Options:
  --keep-inbox    Don't archive inbox (delete it)
  --force         Skip confirmation prompts
```

**Examples:**
```bash
# Destroy VM with inbox archival
sudo ./scripts/destroy-vm.sh agent-01

# Destroy VM without archiving inbox
sudo ./scripts/destroy-vm.sh agent-01 --keep-inbox

# Destroy VM without prompts
sudo ./scripts/destroy-vm.sh agent-01 --force
```

**What gets removed:**
- Libvirt domain definition
- VM disk and cloud-init ISO
- SSH key pair
- Agent secret entries
- DHCP reservation

**What gets preserved:**
- Inbox archived to `/srv/agentshare/archived/{vm}-inbox-{timestamp}/` (unless --keep-inbox)
- Logs in systemd journal (until rotated)

### 4. reprovision-vm.sh (Rebuild)

**Location:** `scripts/reprovision-vm.sh`

Idempotent destroy + provision + deploy workflow for rebuilding VMs.

**What it does:**
- Phase 1: Destroy existing VM (archives inbox)
- Phase 2: Provision fresh VM (defaults: --agentshare --start --wait)
- Phase 3: Deploy agent binary

**Usage:**
```bash
sudo ./scripts/reprovision-vm.sh <vm-name> [OPTIONS]

Options:
  --skip-agent       Don't deploy agent after provisioning
  --keep-inbox       Don't archive existing inbox
  --no-wait          Don't wait for SSH ready

  # All provision-vm.sh options also accepted:
  --cpus N
  --memory SIZE
  --disk SIZE
  --profile NAME
```

**Examples:**
```bash
# Full reprovision (destroy, provision, deploy)
sudo ./scripts/reprovision-vm.sh agent-01

# Reprovision without deploying agent
sudo ./scripts/reprovision-vm.sh agent-01 --skip-agent

# Reprovision with custom resources
sudo ./scripts/reprovision-vm.sh agent-01 --cpus 8 --memory 16G

# Reprovision without waiting for SSH
sudo ./scripts/reprovision-vm.sh agent-01 --no-wait
```

**Use cases:**
- Testing VM provisioning changes
- Resetting VM to clean state
- Updating VM configuration (CPU, RAM, disk)
- Recovering from corrupted VM state

## Security Model

### SSH Key Model

Each VM has two types of SSH access:

**agent user:**
- Ephemeral ed25519 key (automation) - generated at provision time
- User's debug key (interactive access) - from `~/.ssh/id_ed25519.pub`

**root user:**
- User's debug key only (emergency access)
- No automated login allowed

**Key rotation:**
- Ephemeral keys rotate on every provisioning
- User debug keys persist across reprovisioning

**Storage location:**
- `/var/lib/agentic-sandbox/secrets/ssh-keys/<vm-name>` (private key)
- `/var/lib/agentic-sandbox/secrets/ssh-keys/<vm-name>.pub` (public key)

**Access:**
```bash
# Automated access (scripts)
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>

# Interactive access (user)
ssh agent@<IP>
```

### Agent Authentication

**Secret generation:**
- Each VM gets a 256-bit ephemeral secret at provisioning time
- Generated with: `openssl rand -hex 32`
- Secret written to VM via cloud-init at `/etc/agentic-sandbox/agent.env`

**Hash storage on host:**
SHA256 hash stored in two formats:
1. `agent-hashes.json` (JSON) - Read by management server
2. `agent-tokens` (text) - Legacy format for scripting

Example `agent-hashes.json`:
```json
{
  "agent-01": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "agent-02": "a4d8f3b7c2e1095afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
}
```

Example `agent-tokens`:
```
agent-01:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
agent-02:a4d8f3b7c2e1095afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

**Authentication flow:**
1. Agent reads secret from `/etc/agentic-sandbox/agent.env`
2. Agent connects to management server via gRPC
3. Agent sends ID and secret in connection metadata
4. Server hashes secret with SHA256
5. Server compares hash against `agent-hashes.json`
6. Connection accepted if hash matches

**Development mode:**
- Server auto-registers unknown agents on first connect
- Useful for development and testing
- Disable in production environments

### VM Isolation

**Storage isolation:**
- Each VM has its own qcow2 overlay disk
- Each VM has its own ephemeral secrets
- Each VM has its own dedicated inbox directory

**virtiofs access control:**
- Global mount is read-only (enforced by mount options + global-ro symlink)
- Inbox mount is read-write but per-VM isolated
- No VM can access another VM's inbox

**Systemd unit hardening:**
The agent service runs with security restrictions:
- `User=agent` - Non-root execution
- `NoNewPrivileges=true` - Cannot escalate privileges
- `ProtectSystem=strict` - System directories read-only
- `ProtectHome=read-only` - Home directories read-only except /home/agent
- `ReadWritePaths=/home/agent /tmp /mnt/inbox` - Explicit write locations
- `PrivateTmp=true` - Isolated /tmp
- `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX` - Limited network access

**Network isolation:**
- VMs use NAT network (virbr0)
- VMs cannot directly access host except via gRPC
- VMs can access external network (internet)

## Agentshare Storage

### Host Layout

```
/srv/agentshare/
├── global/           # Shared read-only content
│   ├── README.md
│   ├── configs/      # Shared configuration files
│   ├── content/      # Reference content
│   ├── prompts/      # Agent prompt templates
│   ├── scripts/      # Shared utility scripts
│   └── tools/        # Shared tools/binaries
├── global-ro -> global  # Symlink (virtiofs doesn't support <readonly/> in libvirt XML)
├── staging/          # Pre-publish staging area
├── {vm}-inbox/       # Per-VM inbox (read-write)
│   ├── outputs/      # Agent output files
│   ├── logs/         # Agent logs
│   └── runs/         # Per-run directories
│       └── run-{timestamp}/
└── archived/         # Archived inboxes from destroyed VMs
    └── {vm}-inbox-{timestamp}/
```

### VM Mount Points

| Host Path | VM Path | Access | Tag | Options |
|-----------|---------|--------|-----|---------|
| `/srv/agentshare/global-ro` | `/mnt/global` | Read-only | agentglobal | `ro,noatime` |
| `/srv/agentshare/{vm}-inbox` | `/mnt/inbox` | Read-write | agentinbox | `rw,noatime` |

**Convenience symlinks in VM:**
- `~/global` → `/mnt/global`
- `~/inbox` → `/mnt/inbox`

### virtiofs Read-Only Enforcement

libvirt does NOT support `<readonly/>` on virtiofs filesystems. Read-only enforcement uses:

1. **Cloud-init mount options:** `ro,noatime`
2. **global-ro symlink:** Points to `global/` (same content, mount tagged as RO)

Why this approach:
- `<readonly/>` tag in libvirt XML causes "read-only mode not supported" error
- Mount options provide kernel-level enforcement
- Symlink allows single content directory with different mount semantics

### Content Publishing Workflow

**Development:**
1. Create content in `/srv/agentshare/staging/`
2. Test with single VM using custom mount
3. Move to `/srv/agentshare/global/` when ready

**Update process:**
```bash
# Add new content to staging
cp new-tool.sh /srv/agentshare/staging/tools/

# Test with dev VM
# (mount staging instead of global)

# Publish to all VMs
mv /srv/agentshare/staging/tools/new-tool.sh /srv/agentshare/global/tools/

# Content immediately available to all VMs
```

**Important:** virtiofs provides live updates. Changes to global/ are immediately visible in all running VMs.

### Inbox Management

**Per-VM inbox creation:**
Automatically created during provisioning:
```bash
mkdir -p /srv/agentshare/agent-01-inbox/{outputs,logs,runs}
chmod 777 /srv/agentshare/agent-01-inbox
```

**Permissions:**
- Mode: 777 (world read-write-execute)
- Needed for virtiofs uid/gid mapping across host/guest boundary
- Each VM only has access to its own inbox (libvirt domain configuration)

**Cleanup policy:**
- Archived on VM destruction (timestamped)
- Archives retained indefinitely (manual cleanup)
- Location: `/srv/agentshare/archived/{vm}-inbox-{timestamp}/`

## Management Server

### Port Assignments

| Port | Protocol | Purpose |
|------|----------|---------|
| 8120 | gRPC | Agent registration, command dispatch, output streaming |
| 8121 | WebSocket | Browser dashboard real-time updates |
| 8122 | HTTP | Dashboard UI + REST API |

### gRPC API (Port 8120)

**Service:** `AgentService`

**Methods:**
- `RegisterAgent(stream AgentMessage) returns (stream ServerCommand)`
- Bidirectional streaming for command dispatch and output collection

**Agent → Server messages:**
- Connection with ID and secret
- Heartbeat with metrics (every 30s)
- Command output (stdout/stderr)
- Command completion status

**Server → Agent messages:**
- Command execution requests
- Shutdown signals
- Configuration updates

### REST API (Port 8122)

**Endpoints:**

**GET /api/v1/health**
- Health check endpoint
- Returns server status and uptime

**GET /api/v1/agents**
- List all connected agents
- Returns agent metadata, metrics, and system info

Response format:
```json
{
  "agents": [
    {
      "id": "agent-01",
      "connected_at": "2026-01-27T10:00:00Z",
      "last_heartbeat": "2026-01-27T10:05:00Z",
      "metrics": {
        "cpu_usage": 25.3,
        "memory_usage": 45.7,
        "disk_usage": 12.1
      },
      "system_info": {
        "hostname": "agent-01",
        "os": "Ubuntu 24.04",
        "arch": "x86_64"
      }
    }
  ]
}
```

### WebSocket API (Port 8121)

**Connection:** `ws://localhost:8121/ws`

**Client → Server:**
```json
{
  "type": "subscribe",
  "agent_id": "agent-01"
}
```

**Server → Client message types:**

**OutputUpdate:**
```json
{
  "type": "output",
  "agent_id": "agent-01",
  "stream": "stdout",
  "data": "command output...",
  "timestamp": "2026-01-27T10:00:00Z"
}
```

**MetricsUpdate:**
```json
{
  "type": "metrics",
  "agent_id": "agent-01",
  "cpu": 25.3,
  "memory": 45.7,
  "disk": 12.1,
  "timestamp": "2026-01-27T10:00:00Z"
}
```

**AgentConnected:**
```json
{
  "type": "agent_connected",
  "agent_id": "agent-01",
  "timestamp": "2026-01-27T10:00:00Z"
}
```

**AgentDisconnected:**
```json
{
  "type": "agent_disconnected",
  "agent_id": "agent-01",
  "timestamp": "2026-01-27T10:00:00Z"
}
```

### Dashboard Features

**Agent Terminal Panes:**
- xterm.js-based terminal for each connected agent
- Real-time output streaming (stdout/stderr)
- Color-coded output (stdout: white, stderr: red)
- Command input bar per agent
- Command history

**Metrics Display:**
- CPU usage with color-coded thresholds (green < 60%, yellow < 80%, red >= 80%)
- Memory usage with same thresholds
- Disk usage with same thresholds
- Updates every 30 seconds via WebSocket

**Empty State:**
- Shown when no agents connected
- Displays server info and connection instructions
- Auto-refreshes when agents connect

**OAuth Helper:**
- Detects OAuth URLs in agent output
- Shows modal with instructions for token callback
- Simplifies OAuth flows for agents

### Metrics Pipeline

```
Agent VM
  └─> gRPC heartbeat (every 30s)
       └─> Management Server Agent Registry
            └─> Output Aggregator
                 └─> Tag as __metrics__
                      └─> WebSocket connection
                           └─> MetricsUpdate message
                                └─> Dashboard UI
```

**Metric collection:**
- Agent collects system metrics every 30 seconds
- Metrics sent via gRPC heartbeat
- Server tags metrics with `__metrics__` for filtering
- WebSocket streams to subscribed dashboard clients
- Dashboard updates UI with latest values

## Secrets Management

### Directory Layout

```
/var/lib/agentic-sandbox/secrets/
├── agent-hashes.json     # {"agent-id": "sha256_hash", ...} - mgmt server reads this
├── agent-tokens          # agent-id:sha256_hash (one per line) - legacy text format
└── ssh-keys/
    ├── agent-test-01     # Private ed25519 key (ephemeral)
    └── agent-test-01.pub # Public key (injected into VM cloud-init)
```

### agent-hashes.json Format

**Purpose:** Management server authentication database

**Format:**
```json
{
  "agent-01": "sha256_hash_of_secret",
  "agent-02": "sha256_hash_of_secret"
}
```

**Management:**
- Written by `provision-vm.sh`
- Read by management server on agent connect
- Cleaned by `destroy-vm.sh`

**Access control:**
- Mode: 600 (owner read-write only)
- Owner: root

### agent-tokens Format

**Purpose:** Legacy text format for scripting

**Format:**
```
agent-01:sha256_hash_of_secret
agent-02:sha256_hash_of_secret
```

**Management:**
- Written by `provision-vm.sh`
- Can be parsed by shell scripts with `cut -d: -f2`
- Cleaned by `destroy-vm.sh`

### SSH Key Management

**Key generation:**
```bash
ssh-keygen -t ed25519 -f /var/lib/agentic-sandbox/secrets/ssh-keys/{vm-name} -N "" -C "agentic-sandbox-{vm-name}"
```

**Injection into VM:**
- Public key included in cloud-init `user-data`
- Added to `/home/agent/.ssh/authorized_keys`

**Usage:**
```bash
# Automated scripts
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>

# Disable host key checking (ephemeral VMs)
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  agent@<IP>
```

### Secret Rotation

**On provisioning:**
- New 256-bit agent secret generated
- New ed25519 SSH key pair generated
- Old secrets removed from tracking files

**On destruction:**
- SSH key pair deleted
- Agent secret removed from `agent-hashes.json` and `agent-tokens`

**Manual rotation:**
```bash
# Reprovision VM (generates new secrets)
sudo ./scripts/reprovision-vm.sh agent-01
```

## Common Operations

### Deploy New VM Agent

**Full workflow:**
```bash
# 1. Provision VM with agentshare
sudo ./images/qemu/provision-vm.sh --agentshare --start --wait agent-01

# 2. Build agent binary (if not already built)
cd agent-rs && cargo build --release

# 3. Deploy agent binary
sudo ./scripts/provision-vm-agent.sh agent-01

# 4. Verify on dashboard
# Open http://localhost:8122
# Agent should appear with terminal pane and metrics
```

### Rebuild Existing VM (Idempotent)

**Single command:**
```bash
sudo ./scripts/reprovision-vm.sh agent-01
```

**What happens:**
1. Destroy existing VM (archives inbox to `/srv/agentshare/archived/`)
2. Provision fresh VM with same name
3. Deploy agent binary
4. Start agent service

**Use when:**
- Testing provisioning script changes
- VM is in corrupted state
- Need to reset VM to clean slate
- Updating VM configuration

### Update Agent Binary Only

**Workflow:**
```bash
# 1. Make code changes
vim agent-rs/src/main.rs

# 2. Rebuild binary
cd agent-rs && cargo build --release

# 3. Deploy to VM (overwrites existing)
sudo ./scripts/provision-vm-agent.sh agent-01 --force

# 4. Verify in logs
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client -f
```

**No VM restart needed:**
- Systemd automatically restarts service on binary change
- `RestartSec=5s` provides brief delay for stability
- Agent reconnects to management server automatically

### SSH Into VM

**Using ephemeral key (automation):**
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>
```

**Using your debug key (interactive):**
```bash
ssh agent@<IP>
```

**Root access (emergency):**
```bash
ssh root@<IP>
```

**Get VM IP:**
```bash
# From libvirt
virsh domifaddr agent-01

# From IP registry
cat /var/lib/agentic-sandbox/vm-ip-registry.json
```

### View Agent Logs

**Real-time logs:**
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client -f
```

**Recent logs:**
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client -n 100
```

**Logs since time:**
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client --since "10 minutes ago"
```

### View Agent Output on Dashboard

**Access dashboard:**
```
http://localhost:8122
```

**Features:**
- Real-time terminal output per agent
- Metrics display (CPU, memory, disk)
- Command input box
- Agent connection status

### Check Agent Status

**From management server:**
```bash
# REST API
curl http://localhost:8122/api/v1/agents | jq

# WebSocket (using wscat)
wscat -c ws://localhost:8121/ws
```

**From VM:**
```bash
# Service status
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  systemctl status agent-client

# Connection status (check logs for "Connected to server")
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client | grep -i connected
```

### Destroy VM Cleanly

**With inbox archival:**
```bash
sudo ./scripts/destroy-vm.sh agent-01
```

**Without inbox archival:**
```bash
sudo ./scripts/destroy-vm.sh agent-01 --keep-inbox
```

**Force without prompts:**
```bash
sudo ./scripts/destroy-vm.sh agent-01 --force
```

**Verify cleanup:**
```bash
# VM should not be listed
virsh list --all | grep agent-01

# Secrets should be removed
cat /var/lib/agentic-sandbox/secrets/agent-hashes.json | jq

# Inbox should be archived (if not --keep-inbox)
ls -la /srv/agentshare/archived/
```

### Access Shared Content

**From host:**
```bash
# View global content
ls -la /srv/agentshare/global/

# Add new tool
cp mytool.sh /srv/agentshare/global/tools/
chmod +x /srv/agentshare/global/tools/mytool.sh

# Changes immediately visible to all VMs
```

**From VM:**
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>

# Access via mount point
ls -la /mnt/global/

# Access via convenience symlink
ls -la ~/global/

# Execute shared tool
~/global/tools/mytool.sh
```

### Write to VM Inbox

**From VM:**
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>

# Write to inbox
echo "results" > ~/inbox/outputs/result.txt

# Create run directory
mkdir -p ~/inbox/runs/run-$(date +%s)
```

**From host (post-processing):**
```bash
# Read agent output
cat /srv/agentshare/agent-01-inbox/outputs/result.txt

# Process all runs
for run in /srv/agentshare/agent-01-inbox/runs/run-*; do
  echo "Processing $run"
  # ... processing logic ...
done
```

## Troubleshooting

### VM Has No Network

**Symptoms:**
- VM boots but cannot reach management server
- No IP address assigned
- `ip addr` shows interface down

**Cause:**
Cloud-init network-config used hardcoded interface name (e.g., `enp1s0`) but actual interface has different name due to PCI bus assignment variance.

**Diagnosis:**
```bash
# SSH into VM (if possible)
ssh agent@<IP>

# Check interface name
ip link show

# Check netplan config
cat /etc/netplan/50-cloud-init.yaml
```

**Fix:**
Use MAC address matching in netplan instead of interface name.

Update cloud-init network-config template:
```yaml
network:
  version: 2
  ethernets:
    eth0:
      match:
        macaddress: "${MAC_ADDRESS}"
      addresses:
        - ${IP_ADDRESS}/24
      gateway4: 192.168.122.1
      nameservers:
        addresses:
          - 8.8.8.8
          - 8.8.4.4
```

**Prevention:**
- `provision-vm.sh` already uses MAC matching
- Verify cloud-init template if issue persists

### Agent Reports "Invalid Agent Secret"

**Symptoms:**
- Agent connects to server but immediately disconnects
- Management server logs show "Authentication failed"
- Agent logs show "Invalid agent secret"

**Cause:**
`agent-hashes.json` is missing, corrupted, or has wrong hash for the agent.

**Diagnosis:**
```bash
# Check agent-hashes.json
sudo cat /var/lib/agentic-sandbox/secrets/agent-hashes.json | jq

# Check if agent-01 entry exists
sudo cat /var/lib/agentic-sandbox/secrets/agent-hashes.json | jq '.["agent-01"]'

# Check agent.env in VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo cat /etc/agentic-sandbox/agent.env
```

**Fix:**
Reprovision the VM to regenerate secrets:
```bash
sudo ./scripts/reprovision-vm.sh agent-01
```

**Alternative (manual secret sync):**
```bash
# Get secret from VM
VM_SECRET=$(sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  'sudo grep AGENT_SECRET /etc/agentic-sandbox/agent.env | cut -d= -f2')

# Hash it
SECRET_HASH=$(echo -n "$VM_SECRET" | sha256sum | cut -d' ' -f1)

# Update agent-hashes.json
sudo jq --arg id "agent-01" --arg hash "$SECRET_HASH" '.[$id] = $hash' \
  /var/lib/agentic-sandbox/secrets/agent-hashes.json > /tmp/agent-hashes.json.tmp
sudo mv /tmp/agent-hashes.json.tmp /var/lib/agentic-sandbox/secrets/agent-hashes.json

# Restart management server to reload hashes
sudo systemctl restart management-server
```

### SSH Permission Denied

**Symptoms:**
- Cannot SSH into VM with ephemeral key
- "Permission denied (publickey)" error

**Causes:**

**Cause 1: Old host key cached**
VM was reprovisioned and SSH client remembers old host key.

**Fix:**
```bash
# Remove old host key
ssh-keygen -R <VM-IP>

# Or remove entire known_hosts (if using ephemeral VMs only)
rm ~/.ssh/known_hosts

# Retry SSH
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>
```

**Cause 2: Wrong key permissions**
Private key file has wrong permissions.

**Fix:**
```bash
# Fix permissions
sudo chmod 600 /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01

# Retry SSH
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP>
```

**Cause 3: Cloud-init not complete**
VM still booting, SSH key not yet installed.

**Diagnosis:**
```bash
# Check cloud-init status
virsh console agent-01
# Login as user with password (if configured)
cloud-init status
```

**Fix:**
Wait for cloud-init to complete, or use `--wait` flag in provision script.

### virtiofs "Read-Only Mode Not Supported"

**Symptoms:**
- VM fails to start
- libvirt logs show: "virtiofsd: read-only mode not supported"
- Domain XML includes `<readonly/>` in filesystem definition

**Cause:**
virtiofs does NOT support the `<readonly/>` tag in libvirt XML.

**Fix:**
Remove `<readonly/>` from filesystem definition:

**Before (broken):**
```xml
<filesystem type="mount" accessmode="passthrough">
  <driver type="virtiofs"/>
  <source dir="/srv/agentshare/global-ro"/>
  <target dir="agentglobal"/>
  <readonly/>  <!-- REMOVE THIS -->
</filesystem>
```

**After (working):**
```xml
<filesystem type="mount" accessmode="passthrough">
  <driver type="virtiofs"/>
  <source dir="/srv/agentshare/global-ro"/>
  <target dir="agentglobal"/>
</filesystem>
```

**Read-only enforcement:**
Achieved via cloud-init mount options:
```yaml
mounts:
  - [ agentglobal, /mnt/global, virtiofs, "ro,noatime", 0, 0 ]
```

**Prevention:**
- `provision-vm.sh` already omits `<readonly/>`
- Uses `global-ro` symlink + mount options for RO enforcement

### Agent Service Fails With "Permission Denied" Creating Run Directory

**Symptoms:**
- Agent service fails to start
- Logs show: "Permission denied" when creating directories in inbox
- Service can read from global but not write to inbox

**Cause:**
virtiofs permissions mismatch between host and guest.

**Diagnosis:**
```bash
# Check inbox permissions on host
ls -la /srv/agentshare/agent-01-inbox

# Check inbox permissions in VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  ls -la /mnt/inbox
```

**Fix:**
Ensure inbox directory has world-writable permissions on host:
```bash
# Fix permissions
sudo chmod 777 /srv/agentshare/agent-01-inbox
sudo chmod -R 777 /srv/agentshare/agent-01-inbox/*

# Restart agent service
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo systemctl restart agent-client
```

**Prevention:**
- `provision-vm.sh` already creates inbox with 777 permissions
- Verify script if issue persists

### Binary Too Large for qemu-guest-agent

**Symptoms:**
- Agent binary deployment fails
- Error: "Payload too large" or similar
- Using qemu-guest-agent for file transfer

**Cause:**
Old provisioning method used qemu-guest-agent which has size limits (typically 8MB).

**Solution:**
Use new `provision-vm-agent.sh` which deploys via SSH/SCP (no size limits):
```bash
sudo ./scripts/provision-vm-agent.sh agent-01
```

**Details:**
- Old method: `virsh guest-agent-exec` with base64 encoding
- New method: SCP over SSH with ephemeral key
- No size limits with SCP approach
- Faster and more reliable

### Management Server Not Receiving Heartbeats

**Symptoms:**
- Agent appears connected but no metrics updates
- Dashboard shows "No metrics" or stale data
- Metrics timestamp not updating

**Diagnosis:**
```bash
# Check agent logs for heartbeat messages
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client | grep -i heartbeat

# Check management server logs
sudo journalctl -u management-server | grep -i heartbeat

# Check WebSocket messages in browser DevTools
# Should see MetricsUpdate messages every 30s
```

**Possible causes:**

**Cause 1: Agent metrics collection failing**
```bash
# Check if metrics tools installed in VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  which top df free
```

**Fix:**
Reprovision VM with correct cloud-init profile.

**Cause 2: gRPC connection issue**
```bash
# Check if agent can reach management server
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  nc -zv <MGMT_SERVER_IP> 8120
```

**Fix:**
Check network connectivity and firewall rules.

**Cause 3: Metrics parsing error**
```bash
# Check agent logs for errors
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl -u agent-client -n 100 | grep -i error
```

**Fix:**
Update agent code to handle metrics collection errors.

### VM Disk Full

**Symptoms:**
- Agent service crashes
- Cannot write to inbox
- Logs show "No space left on device"

**Diagnosis:**
```bash
# Check disk usage in VM
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  df -h

# Check largest files
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo du -h /home/agent | sort -rh | head -20
```

**Quick fix (free space):**
```bash
# Clean package cache
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo apt-get clean

# Clean old logs
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  sudo journalctl --vacuum-time=7d

# Clean inbox if large
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  rm -rf ~/inbox/runs/run-*
```

**Long-term fix (increase disk size):**
```bash
# Destroy and reprovision with larger disk
sudo ./scripts/reprovision-vm.sh agent-01 --disk 100G
```

### Agent Disconnects Randomly

**Symptoms:**
- Agent connects successfully but disconnects after minutes/hours
- No obvious errors in logs
- Reconnects automatically

**Possible causes:**

**Cause 1: Network timeout**
gRPC connection idle timeout.

**Fix:**
Ensure agent sends heartbeats regularly (every 30s).

**Cause 2: Management server restart**
Server restarted, agents reconnect.

**Expected behavior:** Agents auto-reconnect, no fix needed.

**Cause 3: VM resource exhaustion**
VM running out of memory or CPU.

**Diagnosis:**
Check metrics on dashboard or logs:
```bash
sudo ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<IP> \
  free -h
```

**Fix:**
Increase VM resources:
```bash
sudo ./scripts/reprovision-vm.sh agent-01 --cpus 8 --memory 16G
```

---

**Last updated:** 2026-01-27
**Version:** 1.0

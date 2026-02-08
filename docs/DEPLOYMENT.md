# Deployment Guide

Comprehensive deployment guide for the agentic-sandbox VM orchestration platform. This guide covers installation, configuration, and production deployment from scratch.

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Installation](#installation)
3. [Host Configuration](#host-configuration)
4. [Base Image Setup](#base-image-setup)
5. [Management Server Setup](#management-server-setup)
6. [VM Provisioning](#vm-provisioning)
7. [Agent Deployment](#agent-deployment)
8. [Monitoring Setup](#monitoring-setup)
9. [Production Checklist](#production-checklist)
10. [Verification](#verification)

## Prerequisites

### Hardware Requirements

| Component | Minimum | Recommended | Notes |
|-----------|---------|-------------|-------|
| CPU | 4 cores with KVM support | 16+ cores | Check: `egrep -c '(vmx|svm)' /proc/cpuinfo` |
| RAM | 16GB | 64GB+ | 8GB per agent VM + 8GB for host |
| Disk | 200GB | 1TB+ | SSD strongly recommended |
| Network | 1Gbps NIC | 10Gbps NIC | For agentshare storage performance |

**KVM Support Check:**
```bash
# Check for hardware virtualization support
egrep -c '(vmx|svm)' /proc/cpuinfo
# Should return > 0

# Check if KVM modules are loaded
lsmod | grep kvm
# Should show kvm_intel or kvm_amd

# Check /dev/kvm exists
ls -l /dev/kvm
# Should exist with permissions for kvm or libvirt group
```

### Software Dependencies

**Ubuntu 24.04 LTS (Recommended):**

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install QEMU/KVM and libvirt
sudo apt install -y \
    qemu-kvm \
    libvirt-daemon-system \
    libvirt-clients \
    libvirt-daemon \
    bridge-utils \
    virt-manager \
    cpu-checker

# Verify KVM installation
sudo kvm-ok
# Should output: "KVM acceleration can be used"

# Install build tools
sudo apt install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    protobuf-compiler

# Install Rust (latest stable)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustc --version  # Verify Rust 1.75+

# Install Python 3.11+ for tests
sudo apt install -y python3 python3-pip python3-venv

# Install jq for JSON processing
sudo apt install -y jq
```

**Add User to libvirt and kvm Groups:**

```bash
sudo usermod -aG libvirt,kvm $USER
newgrp libvirt
# Log out and log back in to apply group changes
```

### Network Requirements

| Port | Protocol | Direction | Purpose |
|------|----------|-----------|---------|
| 8120 | TCP | Inbound | gRPC agent connections |
| 8121 | TCP | Inbound | WebSocket streaming |
| 8122 | TCP | Inbound | HTTP dashboard and REST API |
| 9090 | TCP | Localhost | Prometheus metrics |
| 9093 | TCP | Localhost | Alertmanager |
| 3000 | TCP | Localhost | Grafana dashboard |
| 9100 | TCP | VM network | Node exporter (agent VMs) |

**Firewall Configuration (if using UFW):**

```bash
# Allow management server ports
sudo ufw allow 8120/tcp comment 'Agentic Sandbox gRPC'
sudo ufw allow 8121/tcp comment 'Agentic Sandbox WebSocket'
sudo ufw allow 8122/tcp comment 'Agentic Sandbox HTTP'

# Prometheus/Grafana (optional - restrict to localhost)
sudo ufw allow from 127.0.0.1 to any port 9090 proto tcp
sudo ufw allow from 127.0.0.1 to any port 3000 proto tcp

sudo ufw reload
```

## Installation

### 1. Clone Repository

```bash
cd ~/dev  # or your preferred development directory
git clone https://git.integrolabs.net/roctinam/agentic-sandbox.git
cd agentic-sandbox
```

### 2. Build Management Server

```bash
cd management
cargo build --release

# Binary will be at: target/release/agentic-mgmt
# Verify build
./target/release/agentic-mgmt --version
```

**Build time:** Approximately 45 seconds on modern hardware.

### 3. Build Agent Client

```bash
cd ../agent-rs
cargo build --release

# Binary will be at: target/release/agent-client
# Verify build
./target/release/agent-client --version
```

**Build time:** Approximately 30 seconds.

### 4. Build CLI (Optional)

```bash
cd ../cli
cargo build --release

# Binary will be at: target/release/agentic-sandbox
./target/release/agentic-sandbox --help
```

### 5. Verify Installation

```bash
cd ..
tree -L 2 -I target

# Expected structure:
# .
# ├── management/         (Management server)
# ├── agent-rs/          (Agent client)
# ├── cli/               (CLI tool)
# ├── proto/             (gRPC definitions)
# ├── images/qemu/       (VM provisioning)
# ├── scripts/           (Utilities)
# ├── docs/              (Documentation)
# └── tests/             (E2E tests)
```

## Host Configuration

### 1. Directory Structure Setup

Create the required directories for VM storage and shared filesystems:

```bash
# VM storage directory
sudo mkdir -p /var/lib/agentic-sandbox/vms
sudo mkdir -p /var/lib/agentic-sandbox/secrets

# Agentshare storage (virtiofs)
sudo mkdir -p /srv/agentshare/global
sudo mkdir -p /srv/agentshare/inbox
sudo mkdir -p /srv/agentshare/tasks

# Base images storage
sudo mkdir -p /mnt/ops/base-images

# Verify directory structure
tree -L 2 /var/lib/agentic-sandbox /srv/agentshare
```

### 2. Set Permissions

```bash
# VM storage (accessible by libvirt-qemu user)
sudo chown -R libvirt-qemu:kvm /var/lib/agentic-sandbox/vms
sudo chmod 755 /var/lib/agentic-sandbox/vms

# Secrets directory (readable by management server)
sudo chown $USER:$USER /var/lib/agentic-sandbox/secrets
sudo chmod 755 /var/lib/agentic-sandbox/secrets

# Agentshare storage
sudo chown -R $USER:$USER /srv/agentshare
sudo chmod 755 /srv/agentshare

# Global directory (read-only for VMs)
sudo chmod 755 /srv/agentshare/global

# Base images (readable by libvirt)
sudo chown $USER:libvirt-qemu /mnt/ops/base-images
sudo chmod 755 /mnt/ops/base-images
```

### 3. Configure libvirt Network

Verify the default libvirt network is configured:

```bash
# Check if 'default' network exists
virsh net-list --all

# Expected output:
# Name      State    Autostart   Persistent
# --------------------------------------------
# default   active   yes         yes

# If not active, start it
virsh net-start default
virsh net-autostart default

# Verify network configuration
virsh net-dumpxml default

# Expected: 192.168.122.0/24 network with DHCP
```

**Custom Network Configuration (Optional):**

If you need to customize the network range:

```bash
# Define custom network
cat > /tmp/agentic-network.xml <<EOF
<network>
  <name>agentic</name>
  <forward mode='nat'/>
  <bridge name='virbr1' stp='on' delay='0'/>
  <ip address='192.168.122.1' netmask='255.255.255.0'>
    <dhcp>
      <range start='192.168.122.201' end='192.168.122.254'/>
    </dhcp>
  </ip>
</network>
EOF

virsh net-define /tmp/agentic-network.xml
virsh net-start agentic
virsh net-autostart agentic
```

### 4. Configure Storage Pools

```bash
# Define VM storage pool
virsh pool-define-as agentic-vms \
  dir \
  --target /var/lib/agentic-sandbox/vms

virsh pool-start agentic-vms
virsh pool-autostart agentic-vms

# Verify storage pool
virsh pool-list --all
```

### 5. Configure Host Networking (for virtiofs)

Ensure the host can be reached from VMs at the standard gateway address:

```bash
# Check libvirt bridge IP
ip addr show virbr0

# Should show 192.168.122.1
# If using custom network, adjust accordingly
```

## Base Image Setup

### 1. Download Ubuntu 24.04 Cloud Image

```bash
cd /mnt/ops/base-images

# Download Ubuntu 24.04 LTS cloud image
wget https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img

# Rename to match provisioning script expectations
mv ubuntu-24.04-server-cloudimg-amd64.img ubuntu-server-24.04-agent.qcow2

# Verify image
qemu-img info ubuntu-server-24.04-agent.qcow2
```

**Expected output:**
```
image: ubuntu-server-24.04-agent.qcow2
file format: qcow2
virtual size: 2.2 GiB
disk size: 600 MiB
```

### 2. Pre-configure Base Image (Optional)

For faster provisioning, you can pre-install common packages:

```bash
# Install virt-customize (part of libguestfs-tools)
sudo apt install -y libguestfs-tools

# Pre-install common packages
sudo virt-customize -a ubuntu-server-24.04-agent.qcow2 \
  --install qemu-guest-agent,cloud-init \
  --run-command 'systemctl enable qemu-guest-agent' \
  --run-command 'cloud-init clean'

# Create a snapshot for safety
cp ubuntu-server-24.04-agent.qcow2 ubuntu-server-24.04-agent-pristine.qcow2
```

### 3. Verify Cloud-init

Ensure the base image supports cloud-init:

```bash
sudo virt-customize -a ubuntu-server-24.04-agent.qcow2 \
  --run-command 'cloud-init --version' \
  --dry-run
# Should output cloud-init version
```

### 4. Set Base Image Permissions

```bash
sudo chmod 644 /mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2
sudo chown libvirt-qemu:kvm /mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2
```

## Management Server Setup

### Configuration Options

The management server can be configured via environment variables.

**Development Configuration (`.run/dev.env`):**

```bash
cd ~/dev/agentic-sandbox/management
mkdir -p .run

cat > .run/dev.env <<EOF
# Management Server Development Configuration

# Listen address (use 0.0.0.0 to accept external connections)
LISTEN_ADDR=0.0.0.0:8120

# Secrets directory (where agent-hashes.json is stored)
SECRETS_DIR=/var/lib/agentic-sandbox/secrets

# Heartbeat timeout (seconds before marking agent disconnected)
HEARTBEAT_TIMEOUT=90

# Logging configuration
RUST_LOG=info
LOG_FORMAT=pretty  # pretty, json, compact

# Metrics
METRICS_ENABLED=true
EOF
```

**Production Configuration:**

For production, use a systemd service with environment file:

```bash
# Create production environment file
sudo mkdir -p /etc/agentic-sandbox

sudo tee /etc/agentic-sandbox/management.env <<EOF
LISTEN_ADDR=0.0.0.0:8120
SECRETS_DIR=/var/lib/agentic-sandbox/secrets
HEARTBEAT_TIMEOUT=90
RUST_LOG=info
LOG_FORMAT=json
METRICS_ENABLED=true
EOF

sudo chmod 600 /etc/agentic-sandbox/management.env
```

### Running in Development Mode

```bash
cd ~/dev/agentic-sandbox/management

# Start server (builds if needed)
./dev.sh

# Server will start on:
# - gRPC:      localhost:8120
# - WebSocket: localhost:8121
# - HTTP:      localhost:8122

# View logs
./dev.sh logs

# Restart server
./dev.sh restart

# Stop server
./dev.sh stop
```

**Verification:**

```bash
# Check if server is running
curl http://localhost:8122/api/v1/health

# Expected output:
# {"status":"healthy","uptime_seconds":42}

# View dashboard
xdg-open http://localhost:8122  # or open in browser
```

### Running in Production (systemd)

**1. Create systemd service file:**

```bash
sudo tee /etc/systemd/system/agentic-mgmt.service <<EOF
[Unit]
Description=Agentic Sandbox Management Server
After=network-online.target
Wants=network-online.target
Documentation=https://git.integrolabs.net/roctinam/agentic-sandbox

[Service]
Type=simple
User=$USER
Group=$USER
WorkingDirectory=$HOME/dev/agentic-sandbox/management
ExecStart=$HOME/dev/agentic-sandbox/management/target/release/agentic-mgmt
Restart=always
RestartSec=5
EnvironmentFile=/etc/agentic-sandbox/management.env

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/var/lib/agentic-sandbox/secrets

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=agentic-mgmt

[Install]
WantedBy=multi-user.target
EOF
```

**2. Enable and start service:**

```bash
sudo systemctl daemon-reload
sudo systemctl enable agentic-mgmt.service
sudo systemctl start agentic-mgmt.service

# Check status
sudo systemctl status agentic-mgmt.service

# View logs
sudo journalctl -u agentic-mgmt.service -f
```

**3. Verify service is accessible:**

```bash
# Health check
curl http://localhost:8122/api/v1/health

# List agents (should be empty initially)
curl http://localhost:8122/api/v1/agents | jq .
```

### Service Ports

| Port | Protocol | Endpoint | Purpose |
|------|----------|----------|---------|
| 8120 | gRPC | `localhost:8120` | Agent client connections (bidirectional streaming) |
| 8121 | WebSocket | `ws://localhost:8121` | Real-time UI updates (metrics, terminal streams) |
| 8122 | HTTP | `http://localhost:8122` | Dashboard, REST API, metrics endpoint |

## VM Provisioning

### Basic VM Provisioning

Provision a minimal VM with default settings:

```bash
cd ~/dev/agentic-sandbox

# Provision with defaults (4 CPUs, 8GB RAM, 40GB disk)
./images/qemu/provision-vm.sh agent-01 --start

# Provision and wait for SSH
./images/qemu/provision-vm.sh agent-02 --start --wait
```

**Output:**
```
[INFO] Generating ephemeral secret for agent-01
[INFO] Generating SSH key pair for agent-01
[INFO] Allocating IP: 192.168.122.201
[INFO] Creating overlay disk from ubuntu-24.04
[INFO] Generating cloud-init configuration
[INFO] Defining VM in libvirt
[INFO] Starting VM agent-01
[OK] VM agent-01 provisioned successfully
     IP: 192.168.122.201
     SSH: ssh agent@192.168.122.201
     Secret stored in: /var/lib/agentic-sandbox/secrets/agent-hashes.json
```

### Provisioning with agentic-dev Profile

The `agentic-dev` profile includes a full development environment:

```bash
./images/qemu/provision-vm.sh agent-01 \
  --profile agentic-dev \
  --agentshare \
  --start \
  --wait-ready
```

**Included in agentic-dev:**
- **Languages:** Python (uv), Node.js (fnm), Go, Rust
- **AI Tools:** Claude Code, Aider, GitHub Copilot CLI
- **CLI Tools:** ripgrep, fd, bat, eza, delta, jq, xh, grpcurl
- **Build Tools:** cmake, ninja, meson, GCC
- **Containers:** Docker (rootless) with compose and buildx
- **Databases:** PostgreSQL, MySQL, Redis, SQLite clients

### Profile Selection

| Profile | Use Case | Provisioning Time | Disk Usage |
|---------|----------|-------------------|------------|
| `basic` | Minimal SSH access | 1-2 minutes | ~2GB |
| `agentic-dev` | Full development environment | 5-10 minutes | ~8GB |

### Resource Allocation

Adjust resources based on workload:

```bash
# High-performance single VM
./images/qemu/provision-vm.sh agent-01 \
  --cpus 8 \
  --memory 16G \
  --disk 100G \
  --profile agentic-dev \
  --agentshare \
  --start

# Multiple concurrent VMs (resource-efficient)
./images/qemu/provision-vm.sh agent-01 --cpus 2 --memory 4G --disk 20G
./images/qemu/provision-vm.sh agent-02 --cpus 2 --memory 4G --disk 20G
./images/qemu/provision-vm.sh agent-03 --cpus 2 --memory 4G --disk 20G
```

**Resource Guidelines:**

| Scenario | CPUs | Memory | Disk | Concurrent VMs |
|----------|------|--------|------|----------------|
| Single high-perf | 8 | 16G | 100G | 1 |
| Default (2-4 VMs) | 4 | 8G | 40G | 2-4 |
| High-density | 2 | 4G | 20G | 8+ |

### Agentshare Storage

Enable shared storage with `--agentshare`:

```bash
./images/qemu/provision-vm.sh agent-01 \
  --profile agentic-dev \
  --agentshare \
  --start
```

**Mounts inside VM:**
```
/mnt/global  → ~/global   (read-only shared resources)
/mnt/inbox   → ~/inbox    (read-write per-agent workspace)
```

**Verify agentshare inside VM:**

```bash
ssh agent@192.168.122.201

# Check mounts
ls -la ~/global ~/inbox

# Test write access
echo "test" > ~/inbox/test.txt
cat ~/inbox/test.txt
```

### Network Modes

Control VM network access:

```bash
# Full network access (default)
./images/qemu/provision-vm.sh agent-01 --network-mode full

# Isolated (management server only)
./images/qemu/provision-vm.sh agent-01 --network-mode isolated

# Allowlist (DNS-filtered HTTPS only)
./images/qemu/provision-vm.sh agent-01 --network-mode allowlist
```

| Mode | Management Server | Internet | DNS | Use Case |
|------|-------------------|----------|-----|----------|
| `full` | Yes | Yes | Yes | Development, unrestricted tasks |
| `isolated` | Yes | No | Limited | High-security, offline tasks |
| `allowlist` | Yes | HTTPS only | Filtered | Production, controlled egress |

### Static IP Allocation

VMs are automatically assigned static IPs based on their name:

| VM Name | IP Address |
|---------|------------|
| agent-01 | 192.168.122.201 |
| agent-02 | 192.168.122.202 |
| agent-03 | 192.168.122.203 |
| ... | ... |
| agent-54 | 192.168.122.254 |

**Manual IP assignment:**

```bash
./images/qemu/provision-vm.sh agent-custom \
  --ip 192.168.122.220 \
  --start
```

### Verify VM Provisioning

```bash
# List all VMs
virsh list --all

# Check VM status
virsh domstate agent-01

# Get VM IP
virsh domifaddr agent-01

# SSH to VM
ssh agent@192.168.122.201

# Inside VM, check agent client
sudo systemctl status agentic-agent

# Check agent logs
sudo journalctl -u agentic-agent -f
```

## Agent Deployment

Agent deployment happens automatically during VM provisioning. To update agents after code changes:

### Deploy Agent to Single VM

```bash
cd ~/dev/agentic-sandbox

# Deploy with normal logging
./scripts/deploy-agent.sh agent-01

# Deploy with debug logging
./scripts/deploy-agent.sh agent-01 --debug

# Force rebuild agent binary
./scripts/deploy-agent.sh agent-01 --rebuild
```

**Output:**
```
[deploy] Deploying to agent-01 (192.168.122.201)
[deploy] Building agent binary...
[deploy] Waiting for SSH...
[deploy] Reading secret from VM...
[deploy] Found secret: 8f3a2b4c1d5e6f...
[deploy] Copying agent binary...
[deploy] Configuring agent service (log_level=info)...
[deploy] Verifying deployment...
[deploy] SUCCESS: Agent deployed and running on agent-01

Feb 07 12:34:56 agent-01 agentic-agent[1234]: INFO Connected to management server
Feb 07 12:34:57 agent-01 agentic-agent[1234]: INFO Heartbeat sent
```

### Deploy to All Running VMs

```bash
# Full rebuild and deploy to all VMs
./scripts/dev-deploy-all.sh

# With debug logging
./scripts/dev-deploy-all.sh --debug
```

This script:
1. Rebuilds management server
2. Restarts management server
3. Rebuilds agent client
4. Deploys agent to all running VMs
5. Verifies all agents reconnect

### Verify Agent Connectivity

```bash
# Check agent status via management server API
curl http://localhost:8122/api/v1/agents | jq .

# Expected output:
# [
#   {
#     "agent_id": "agent-01",
#     "status": "ready",
#     "connected_at": "2026-02-07T12:34:56Z",
#     "last_heartbeat": "2026-02-07T12:35:26Z",
#     "capabilities": ["exec", "file_transfer", "pty"]
#   }
# ]

# Check agent inside VM
ssh agent@192.168.122.201 'sudo systemctl status agentic-agent'
```

### Agent Configuration

Agent configuration is stored in `/etc/agentic-sandbox/agent.env` on each VM:

```bash
# Inside VM
sudo cat /etc/agentic-sandbox/agent.env
```

**Example:**
```bash
AGENT_ID=agent-01
AGENT_SECRET=8f3a2b4c1d5e6f7a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8c9d0e1f2a
MANAGEMENT_SERVER=192.168.122.1:8120
HEARTBEAT_INTERVAL=30
RUST_LOG=info
```

**Security:**
- File is root-owned with mode 600
- Plaintext secret is stored in VM only
- Host stores SHA256 hash in `/var/lib/agentic-sandbox/secrets/agent-hashes.json`

### Troubleshooting Agent Connection

**Agent not connecting:**

```bash
# Check management server is running
curl http://localhost:8122/api/v1/health

# Check agent service status
ssh agent@192.168.122.201 'sudo systemctl status agentic-agent'

# Check agent logs
ssh agent@192.168.122.201 'sudo journalctl -u agentic-agent -n 50'

# Verify secret hash matches
# On host:
jq . /var/lib/agentic-sandbox/secrets/agent-hashes.json

# On VM (get plaintext secret and hash it):
ssh agent@192.168.122.201 'sudo cat /etc/agentic-sandbox/agent.env | grep AGENT_SECRET'
echo -n "<secret>" | sha256sum  # Should match hash in agent-hashes.json
```

**Agent disconnecting:**

```bash
# Check network connectivity
ssh agent@192.168.122.201 'ping -c 3 192.168.122.1'

# Check firewall rules
sudo ufw status

# Increase heartbeat timeout in management server
# Edit /etc/agentic-sandbox/management.env
HEARTBEAT_TIMEOUT=120  # Increase from 90 to 120 seconds
sudo systemctl restart agentic-mgmt
```

## Monitoring Setup

### Prometheus Installation

```bash
# Install Prometheus and Alertmanager
sudo apt update
sudo apt install -y prometheus prometheus-alertmanager

# Enable services
sudo systemctl enable prometheus alertmanager
sudo systemctl start prometheus alertmanager

# Verify installation
prometheus --version
amtool --version
```

### Deploy Observability Stack

```bash
cd ~/dev/agentic-sandbox/scripts/prometheus

# Deploy full stack (Prometheus + Alertmanager + Grafana)
sudo ./deploy.sh
```

**This script will:**
1. Install Prometheus, Alertmanager, and Grafana
2. Deploy configuration files
3. Configure alert rules
4. Start all services
5. Prompt for Slack and PagerDuty configuration

### Configure Alertmanager

**Edit Alertmanager configuration:**

```bash
sudo nano /etc/alertmanager/alertmanager.yml
```

**Add Slack webhook URL:**

```yaml
receivers:
  - name: 'slack-alerts'
    slack_configs:
      - api_url: 'https://hooks.slack.com/services/YOUR/WEBHOOK/URL'
        channel: '#alerts'
        title: 'Agentic Sandbox Alert'
        text: '{{ range .Alerts }}{{ .Annotations.description }}{{ end }}'
```

**Add PagerDuty service key:**

```yaml
  - name: 'pagerduty-critical'
    pagerduty_configs:
      - service_key: 'YOUR_PAGERDUTY_SERVICE_KEY'
        description: '{{ .CommonAnnotations.summary }}'
```

**Restart Alertmanager:**

```bash
sudo systemctl restart alertmanager
```

### Configure Grafana

**1. Access Grafana:**

Open http://localhost:3000 in browser.
- **Default credentials:** admin/admin
- Change password on first login

**2. Add Prometheus data source:**

1. Go to **Configuration** → **Data Sources** → **Add data source**
2. Select **Prometheus**
3. Set URL: `http://localhost:9090`
4. Click **Save & Test**

**3. Import dashboards:**

The prometheus directory includes pre-built dashboard JSON files. Import them via:

1. **Dashboards** → **Import**
2. Upload JSON file or paste JSON
3. Select Prometheus data source
4. Click **Import**

### Enable node_exporter on Agent VMs

Node exporter is automatically installed with the `agentic-dev` profile. For existing VMs:

```bash
# SSH to agent VM
ssh agent@192.168.122.201

# Install node_exporter
sudo apt install -y prometheus-node-exporter

# Create textfile collector directory
sudo mkdir -p /var/lib/prometheus/node-exporter
sudo chown agent:agent /var/lib/prometheus/node-exporter

# Restart node_exporter
sudo systemctl restart prometheus-node-exporter

# Verify metrics endpoint
curl http://localhost:9100/metrics | head -20
```

### Verify Monitoring

**Check Prometheus targets:**

```bash
# Via API
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | {job: .labels.job, health: .health}'

# Via web UI
xdg-open http://localhost:9090/targets
```

**Expected targets:**

| Job | Target | Status |
|-----|--------|--------|
| management-server | localhost:8122 | UP |
| agent-vms | 192.168.122.201:9100 | UP |
| agent-vms | 192.168.122.202:9100 | UP |

**Check metrics are being collected:**

```bash
# Query agent count
curl -G http://localhost:9090/api/v1/query \
  --data-urlencode 'query=agentic_agents_connected' | jq .

# Query command execution rate
curl -G http://localhost:9090/api/v1/query \
  --data-urlencode 'query=rate(agentic_commands_total[5m])' | jq .
```

### Access Monitoring Dashboards

| Service | URL | Purpose |
|---------|-----|---------|
| **Prometheus** | http://localhost:9090 | Metrics query and exploration |
| **Alertmanager** | http://localhost:9093 | Alert management and silencing |
| **Grafana** | http://localhost:3000 | Visualization dashboards |
| **Management Dashboard** | http://localhost:8122 | Live agent status and terminal |

## Production Checklist

### Security Hardening

- [ ] **Firewall configured** - Only expose necessary ports
- [ ] **SSH key authentication** - Disable password authentication
- [ ] **TLS certificates** - Use HTTPS for web dashboard (behind reverse proxy)
- [ ] **Secret rotation** - Rotate agent secrets regularly
- [ ] **Audit logging** - Enable audit logs for all agent actions
- [ ] **Resource quotas** - Set CPU, memory, and disk quotas per VM
- [ ] **Network isolation** - Use `isolated` or `allowlist` network modes
- [ ] **Docker rootless** - Ensure containers run without root privileges

**Enable audit logging:**

```bash
# Edit management.env
sudo nano /etc/agentic-sandbox/management.env

# Add:
AUDIT_LOG_ENABLED=true
AUDIT_LOG_PATH=/var/log/agentic-sandbox/audit.log

# Create log directory
sudo mkdir -p /var/log/agentic-sandbox
sudo chown $USER:$USER /var/log/agentic-sandbox

# Restart management server
sudo systemctl restart agentic-mgmt
```

### Backup Configuration

**Critical files to backup:**

```bash
# Backup script
sudo tee /usr/local/bin/backup-agentic-sandbox.sh <<'EOF'
#!/bin/bash
BACKUP_DIR="/var/backups/agentic-sandbox/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$BACKUP_DIR"

# Secrets (encrypted)
tar -czf "$BACKUP_DIR/secrets.tar.gz.gpg" \
  --transform 's|^|secrets/|' \
  -C /var/lib/agentic-sandbox/secrets .
gpg --symmetric "$BACKUP_DIR/secrets.tar.gz"

# VM definitions
for vm in $(virsh list --all --name | grep agent-); do
  virsh dumpxml "$vm" > "$BACKUP_DIR/${vm}.xml"
done

# Management server config
cp /etc/agentic-sandbox/management.env "$BACKUP_DIR/"

# Prometheus data (optional - large)
# rsync -a /var/lib/prometheus/data/ "$BACKUP_DIR/prometheus/"

echo "Backup completed: $BACKUP_DIR"
EOF

sudo chmod +x /usr/local/bin/backup-agentic-sandbox.sh

# Schedule daily backups
sudo tee /etc/cron.daily/agentic-backup <<EOF
#!/bin/bash
/usr/local/bin/backup-agentic-sandbox.sh >> /var/log/agentic-backup.log 2>&1
EOF
sudo chmod +x /etc/cron.daily/agentic-backup
```

### Log Rotation

```bash
# Configure logrotate
sudo tee /etc/logrotate.d/agentic-sandbox <<EOF
/var/log/agentic-sandbox/*.log {
    daily
    rotate 30
    compress
    delaycompress
    notifempty
    create 0644 $USER $USER
    sharedscripts
    postrotate
        systemctl reload agentic-mgmt > /dev/null 2>&1 || true
    endscript
}
EOF
```

### Resource Limits

Set system-wide resource limits:

```bash
# Edit provisioning defaults
nano ~/dev/agentic-sandbox/images/qemu/provision-vm.sh

# Adjust DEFAULT_* values:
DEFAULT_CPUS="2"        # Reduce for high-density
DEFAULT_MEMORY="4G"     # Reduce for high-density
DEFAULT_DISK="20G"      # Reduce for ephemeral tasks

# Set per-VM limits during provisioning
./images/qemu/provision-vm.sh agent-01 \
  --mem-limit 3800M \
  --cpu-quota 180 \
  --io-read-limit 300M \
  --io-write-limit 100M \
  --disk-quota 10G
```

### Monitoring Alerts

Review and customize alert thresholds:

```bash
sudo nano /etc/prometheus/rules/agentic-sandbox.yml

# Adjust alert thresholds based on your SLOs
# Example: Reduce CPU threshold for production
- alert: AgentHighCPU
  expr: agent_cpu_usage > 70  # Changed from 80
  for: 5m  # Changed from 10m
```

### Capacity Planning

Monitor resource usage to plan capacity:

```bash
# Check current resource usage
virsh domstats --state-running

# Prometheus query for average CPU usage
curl -G http://localhost:9090/api/v1/query \
  --data-urlencode 'query=avg(agent_cpu_usage)'

# Prometheus query for memory usage
curl -G http://localhost:9090/api/v1/query \
  --data-urlencode 'query=avg(agent_memory_usage_percent)'

# Estimate max concurrent VMs
# Available RAM / Average VM RAM = Max VMs
# Example: 64GB host RAM / 8GB per VM = 8 concurrent VMs
```

## Verification

### 1. Health Checks

```bash
# Management server health
curl http://localhost:8122/api/v1/health
# Expected: {"status":"healthy","uptime_seconds":123}

# Management server readiness
curl http://localhost:8122/readyz
# Expected: {"status":"ready","agents_connected":3}

# Prometheus targets
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | {job, health}'
# Expected: All targets with health="up"
```

### 2. Agent Connectivity

```bash
# List connected agents
curl http://localhost:8122/api/v1/agents | jq .

# Expected output (for 3 VMs):
# [
#   {"agent_id": "agent-01", "status": "ready", ...},
#   {"agent_id": "agent-02", "status": "ready", ...},
#   {"agent_id": "agent-03", "status": "ready", ...}
# ]

# Check agent heartbeats
for i in {1..3}; do
  echo "Checking agent-0$i..."
  ssh agent@192.168.122.20$i 'sudo journalctl -u agentic-agent -n 5 --no-pager | grep -i heartbeat'
done
```

### 3. Test Command Execution

```bash
# Execute command on agent
curl -X POST http://localhost:8122/api/v1/agents/agent-01/exec \
  -H "Content-Type: application/json" \
  -d '{
    "command": "echo Hello from agent-01 && uname -a"
  }' | jq .

# Expected: Command output with exit code 0
```

### 4. Test Terminal Session

```bash
# Open dashboard
xdg-open http://localhost:8122

# Click on agent-01
# Click "Terminal" button
# Type commands in terminal

# Verify output appears in real-time
```

### 5. Test Agentshare Storage

```bash
# On host: Create test file in global
echo "Shared resource" > /srv/agentshare/global/test.txt

# In VM: Verify read access
ssh agent@192.168.122.201 'cat ~/global/test.txt'
# Expected: "Shared resource"

# In VM: Test write to inbox
ssh agent@192.168.122.201 'echo "Agent output" > ~/inbox/output.txt'

# On host: Verify file appears
cat /srv/agentshare/inbox/agent-01/output.txt
# Expected: "Agent output"
```

### 6. Submit Test Task

```bash
# Submit a simple task
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Create a Python script that prints Hello World",
    "model": "claude-sonnet-4-20250514",
    "timeout_seconds": 300
  }' | jq .

# Get task ID from response
TASK_ID="<task-id-from-response>"

# Check task status
curl http://localhost:8122/api/v1/tasks/$TASK_ID | jq .

# Stream task logs
curl http://localhost:8122/api/v1/tasks/$TASK_ID/logs

# Wait for task completion, then check artifacts
curl http://localhost:8122/api/v1/tasks/$TASK_ID/artifacts | jq .
```

### 7. Verify Monitoring

```bash
# Check Prometheus metrics
curl http://localhost:8122/metrics | grep agentic_agents_connected
# Expected: agentic_agents_connected 3

# Check alert rules are loaded
curl http://localhost:9090/api/v1/rules | jq '.data.groups[] | .name'

# Verify Grafana dashboards
# Open http://localhost:3000
# Navigate to Dashboards → Browse
# Verify "Agent Fleet Overview" dashboard loads
```

### 8. Full Integration Test

Run the E2E test suite:

```bash
cd ~/dev/agentic-sandbox/tests/e2e

# Install test dependencies
pip install -r requirements.txt

# Run E2E tests
pytest -v

# Expected: All tests pass
```

### 9. Verify VM Lifecycle

```bash
# Stop VM
virsh shutdown agent-01

# Verify agent disconnects
curl http://localhost:8122/api/v1/agents | jq '.[] | select(.agent_id=="agent-01")'
# Expected: status="disconnected" or agent not in list

# Start VM
virsh start agent-01

# Wait 30 seconds for agent to reconnect
sleep 30

# Verify agent reconnects
curl http://localhost:8122/api/v1/agents | jq '.[] | select(.agent_id=="agent-01")'
# Expected: status="ready"
```

### 10. Production Readiness Checklist

Final checklist before production deployment:

- [ ] All services start automatically on boot
- [ ] Health endpoints return 200 OK
- [ ] At least 3 agent VMs provisioned and connected
- [ ] Commands execute successfully on agents
- [ ] Terminal sessions work in dashboard
- [ ] Agentshare storage is accessible (read global, write inbox)
- [ ] Prometheus is scraping all targets
- [ ] Alertmanager is configured with notification channels
- [ ] Grafana dashboards load and display data
- [ ] Backup script runs successfully
- [ ] Log rotation configured
- [ ] Firewall rules tested
- [ ] Audit logging enabled
- [ ] Documentation reviewed and updated
- [ ] Runbooks created for alert responses

## Troubleshooting Common Issues

### Management Server Won't Start

```bash
# Check logs
sudo journalctl -u agentic-mgmt -n 50

# Common issues:
# 1. Port already in use
sudo lsof -i :8120
# Kill conflicting process or change LISTEN_ADDR

# 2. Secrets directory not readable
ls -la /var/lib/agentic-sandbox/secrets
sudo chmod 755 /var/lib/agentic-sandbox/secrets

# 3. Missing environment file
ls -la /etc/agentic-sandbox/management.env
```

### VM Won't Start

```bash
# Check libvirt logs
sudo journalctl -u libvirtd -n 50

# Check VM definition
virsh dumpxml agent-01

# Common issues:
# 1. Base image not found
ls -la /mnt/ops/base-images/

# 2. Overlay disk not found
ls -la /var/lib/agentic-sandbox/vms/agent-01/

# 3. Network not active
virsh net-list --all
virsh net-start default
```

### Agent Won't Connect

See [Agent Deployment - Troubleshooting Agent Connection](#troubleshooting-agent-connection).

### Prometheus Not Scraping Targets

```bash
# Check Prometheus logs
sudo journalctl -u prometheus -n 50

# Verify config syntax
promtool check config /etc/prometheus/prometheus.yml

# Test target connectivity
curl http://localhost:8122/metrics  # Management server
curl http://192.168.122.201:9100/metrics  # Agent VM

# Reload Prometheus config
sudo systemctl reload prometheus
```

## Next Steps

After successful deployment:

1. **Read Operations Guide:** See `docs/OPERATIONS.md` for day-to-day operations
2. **Review API Documentation:** See `docs/API.md` for complete API reference
3. **Set Up CI/CD:** Automate agent deployment with your CI/CD pipeline
4. **Configure Backups:** Set up automated backups for critical data
5. **Create Runbooks:** Document procedures for common operational tasks
6. **Train Team:** Ensure team members understand system architecture and operations

## References

- **Architecture:** `docs/ARCHITECTURE.md`
- **API Reference:** `docs/API.md`
- **Observability:** `docs/OBSERVABILITY_DESIGN.md`
- **Session Management:** `docs/SESSION_RECONCILIATION.md`
- **Build Guide:** `BUILD.md`
- **Project README:** `README.md`

---

**Deployment Guide Version:** 1.0
**Last Updated:** 2026-02-07
**Maintained By:** Agentic Sandbox Team

# QEMU VM Provisioning

Rapidly provision agent VMs from base images with full development environments.

## Quick Start

```bash
# Provision a development VM with full tooling
./provision-vm.sh agent-01 --profile agentic-dev --agentshare --start

# Basic VM (minimal, SSH only)
./provision-vm.sh agent-02 --start

# Wait for full setup to complete
./provision-vm.sh agent-03 --profile agentic-dev --agentshare --wait-ready
```

## Requirements

### Host Packages

```bash
sudo apt install qemu-kvm libvirt-daemon-system libvirt-clients \
    virtinst genisoimage qemu-utils libguestfs-tools
```

### libvirt Setup

```bash
sudo systemctl enable --now libvirtd
sudo usermod -aG libvirt $USER
# Log out and back in for group membership
```

### Base Image

You need a base image before provisioning. Either:

1. **Build one** (see [Building Base Images](#building-base-images)):
   ```bash
   ./build-base-image.sh 24.04
   ```

2. **Or download** pre-built images to `/mnt/ops/base-images/`

### Agentshare (Optional)

For shared storage between VMs:

```bash
# Initialize agentshare directories
sudo mkdir -p /srv/agentshare/{global,global-ro}
sudo chmod 755 /srv/agentshare/global-ro
# global-ro is mounted read-only in VMs
```

## Provisioning Options

```bash
./provision-vm.sh [OPTIONS] NAME
```

| Option | Default | Description |
|--------|---------|-------------|
| `--profile NAME` | basic | Provisioning profile (see below) |
| `--cpus NUM` | 4 | CPU cores |
| `--memory SIZE` | 8G | RAM (e.g., 4G, 8192M) |
| `--disk SIZE` | 40G | Disk size |
| `--ssh-key FILE` | auto-detect | SSH public key file |
| `--start` | false | Start VM immediately |
| `--wait` | false | Wait for SSH ready (implies --start) |
| `--wait-ready` | false | Wait for profile setup (implies --wait) |
| `--agentshare` | false | Enable virtiofs mounts |
| `--ip IP` | auto | Static IP address |
| `--network NET` | default | libvirt network |
| `--management HOST` | host.internal:8120 | Management server address |
| `--dry-run` | false | Show what would be done |

## Profiles

### basic

Minimal environment for simple tasks:
- SSH access with user's key
- qemu-guest-agent
- Basic utilities (curl, wget, git, jq)
- Health check server on port 8118

### agentic-dev (Recommended)

Comprehensive development environment with AI coding tools:

**Languages & Runtimes:**
| Tool | Description |
|------|-------------|
| Python (uv) | Fast package manager, venv, tooling |
| Node.js (fnm) | Fast Node Manager with LTS |
| pnpm | Fast package manager |
| Bun | Fast JS runtime/bundler |
| Go 1.22 | Go runtime at /usr/local/go |
| Rust (rustup) | Rust with clippy, rustfmt, rust-analyzer |
| mise | Universal version manager |

**AI Coding Tools:**
| Tool | Description |
|------|-------------|
| Claude Code | Anthropic's AI coding assistant |
| Aider | AI pair programmer |
| Codex | OpenAI Codex CLI |
| GitHub Copilot CLI | AI-powered shell |
| aiwg | AI Writing Guide |

### Claude Code Authentication

Claude Code is installed in two steps during provisioning:
1. Install via script: `curl -fsSL https://claude.ai/install.sh | bash`
2. Finalize and update: `claude install`

**Authentication options:**
- **OAuth (recommended)**: Run `/login` in Claude Code to authenticate via Anthropic Console
- **API Key**: Set `ANTHROPIC_API_KEY` environment variable

Note: The managed settings at `/etc/claude-code/managed-settings.json` define sandbox permissions. API key helper is not currently available - use OAuth or environment variable.

**CLI Tools:**
| Tool | Description |
|------|-------------|
| ripgrep (rg) | Fast grep |
| fd | Fast find |
| bat | Cat with syntax highlighting |
| eza | Modern ls |
| delta | Git diff viewer |
| jq | JSON processor |
| xh | Modern httpie |
| grpcurl | gRPC CLI |
| websocat | WebSocket CLI |
| hyperfine | Benchmarking |

**Build Systems:**
- cmake, ninja, meson, GCC

**Database Clients:**
- postgresql-client, mysql-client, redis-tools, sqlite3

**Observability:**
- strace, ltrace, sysstat, iotop, nethogs

**Containers:**
- Docker CE with compose and buildx

### GOPATH Configuration

GOPATH is set to `~/.local/go` to keep the home directory clean. Go binaries install to `~/.local/go/bin`, which is in PATH.

### Timezone

VMs are configured with `America/New_York` timezone to match the host. This ensures consistent timestamps for logs, API calls (JWT tokens), and coordination with the management server.

## Agentshare Storage

When `--agentshare` is enabled, VMs get virtiofs mounts:

| VM Path | Home Symlink | Mode | Purpose |
|---------|--------------|------|---------|
| `/mnt/global` | `~/global` | Read-only | Shared resources |
| `/mnt/inbox` | `~/inbox` | Read-write | Per-agent outputs |

The inbox includes:
- `outputs/` - Task outputs
- `logs/` - Agent logs
- `runs/<run-id>/` - Per-run directories
- `current/` - Symlink to latest run

## Security Model

### Ephemeral Secrets

Each VM gets a unique 256-bit secret at provisioning time:

1. **Secret Generation**: 32-byte random hex string created
2. **Hash Storage**: SHA256 hash stored in `/var/lib/agentic-sandbox/secrets/agent-hashes.json`
3. **VM Injection**: Plaintext secret injected into `/etc/agentic-sandbox/agent.env`
4. **Authentication**: Agent client uses secret, management server verifies hash

The plaintext secret never leaves the VM (except via cloud-init one-time injection).

### Ephemeral SSH Keys

Each VM gets a unique SSH key pair:

- **Private key**: Stored at `/var/lib/agentic-sandbox/secrets/ssh-keys/<agent-id>`
- **Public key**: Injected into VM's authorized_keys

This allows automated management access without sharing the user's SSH key.

### User SSH Key

The user's SSH public key is also injected for interactive debugging.

### Secret Locations

| File | Contents |
|------|----------|
| `/var/lib/agentic-sandbox/secrets/agent-hashes.json` | Agent ID → SHA256 hash mapping |
| `/var/lib/agentic-sandbox/secrets/agent-tokens` | Legacy text format (agent:hash) |
| `/var/lib/agentic-sandbox/secrets/ssh-keys/` | Ephemeral SSH key pairs |

## IP Address Management

VMs get static IPs via DHCP reservation:

1. **Pattern-based**: `agent-01` → `192.168.122.201`, `agent-02` → `192.168.122.202`, etc.
2. **Registry**: Allocations tracked in `/var/lib/agentic-sandbox/vms/.ip-registry`
3. **DHCP Reservation**: Added to libvirt network on provisioning
4. **MAC Address**: Deterministic based on VM name

Range: `192.168.122.201` - `192.168.122.254` (54 VMs max)

## VM Lifecycle

### Start/Stop

```bash
# Start VM
virsh start agent-01

# Stop gracefully
virsh shutdown agent-01

# Force stop
virsh destroy agent-01
```

### Connect

```bash
# SSH (after VM starts)
ssh agent@192.168.122.201

# Serial console (always works)
virsh console agent-01
```

### Check Status

```bash
# VM state
virsh dominfo agent-01

# Health check (from host)
curl http://192.168.122.201:8118/health

# Readiness check
curl http://192.168.122.201:8118/ready
```

### Delete

```bash
# Stop if running
virsh destroy agent-01

# Undefine from libvirt
virsh undefine agent-01

# Remove storage
rm -rf /var/lib/agentic-sandbox/vms/agent-01

# Remove DHCP reservation (optional - happens on reprovision)
```

### Reprovision

To rebuild a VM in place (preserves IP allocation):

```bash
# From scripts directory
./scripts/reprovision-vm.sh agent-01 --profile agentic-dev
```

## Health Check Server

VMs run a health server on port 8118:

| Endpoint | Description |
|----------|-------------|
| `/health` | Health status JSON |
| `/ready` | Returns 200 when setup complete, 503 otherwise |
| `/logs/<file>` | Stream log file (e.g., `/logs/syslog`) |
| `/stream/stdout` | Stream agent stdout |
| `/stream/stderr` | Stream agent stderr |

## On-Demand Tool Installation

The agentic-dev profile includes an install facility for additional tools:

```bash
# Inside VM
/opt/agentic-sandbox/install-tool.sh list    # Show available
/opt/agentic-sandbox/install-tool.sh llvm    # Install LLVM/Clang
/opt/agentic-sandbox/install-tool.sh pgcli   # Install enhanced psql
```

Available tools:
- **Languages**: llvm, deno, zig
- **Build**: just, watchexec
- **Database TUI**: pgcli, mycli, litecli
- **Git/Dev**: lazygit, glow
- **Go**: golangci-lint, gopls

## Building Base Images

Base images are minimal Ubuntu Server with cloud-init and qemu-guest-agent.

### Build

```bash
# Build Ubuntu 24.04 base image
./build-base-image.sh 24.04

# Custom disk size
./build-base-image.sh --disk-size 60G 24.04

# Dry run
./build-base-image.sh --dry-run 24.04
```

### Requirements

ISOs in `/mnt/ops/isos/linux/`:
- `ubuntu-24.04.x-live-server-amd64.iso`

### Output

Images created in `/mnt/ops/base-images/`:
- `ubuntu-server-24.04-agent.qcow2`

### Base Image Contents

- Ubuntu Server (minimal)
- qemu-guest-agent
- openssh-server
- cloud-init
- python3
- Common tools (curl, wget, git, jq)

### Default User

- **Username**: `agent`
- **Password**: Disabled (SSH key auth only)
- **Sudo**: Passwordless

## Directory Structure

```
images/qemu/
├── provision-vm.sh      # Main provisioning script
├── build-base-image.sh  # Base image builder
├── autoinstall/         # Autoinstall templates
│   ├── user-data.template
│   └── meta-data.template
├── profiles/            # Profile configurations (future)
└── README.md            # This file

/var/lib/agentic-sandbox/
├── vms/                 # VM storage
│   └── <vm-name>/
│       ├── <vm-name>.qcow2    # Overlay disk
│       ├── cloud-init/        # Cloud-init files
│       ├── cloud-init.iso     # Cloud-init ISO
│       └── vm-info.json       # VM metadata
├── secrets/             # Agent secrets
│   ├── agent-hashes.json      # SHA256 hashes
│   ├── agent-tokens           # Legacy format
│   └── ssh-keys/              # Ephemeral SSH keys
└── .ip-registry         # IP allocations

/srv/agentshare/         # Shared storage
├── global/              # Read-write source
├── global-ro/           # Read-only mount target
└── <vm-name>-inbox/     # Per-agent inbox
```

## Troubleshooting

### VM Won't Start

```bash
# Check libvirt logs
virsh dominfo agent-01
journalctl -u libvirtd --since "10 minutes ago"

# Check XML definition
virsh dumpxml agent-01
```

### No IP Address

```bash
# Check DHCP reservation
virsh net-dumpxml default | grep agent-01

# Check VM has network
virsh domifaddr agent-01

# Check qemu-guest-agent
virsh qemu-agent-command agent-01 '{"execute":"guest-network-get-interfaces"}'
```

### Cloud-init Not Running

```bash
# Connect via console
virsh console agent-01

# Check cloud-init status
cloud-init status
cat /var/log/cloud-init-output.log
```

### Agentshare Not Mounting

```bash
# Check virtiofs in VM
mount | grep virtiofs
cat /etc/fstab | grep virtiofs

# Check host directories exist
ls -la /srv/agentshare/global-ro
ls -la /srv/agentshare/<vm-name>-inbox
```

### Setup Not Completing (agentic-dev)

```bash
# Check setup log in VM
ssh agent@<ip> "tail -f /var/log/agentic-setup.log"

# Check readiness
curl http://<ip>:8118/ready
```

## Resource Guidelines

For concurrent VMs on a typical workstation:

| Scenario | CPUs | Memory | Disk |
|----------|------|--------|------|
| Single VM | 8 | 16G | 80G |
| 2 concurrent | 4 | 8G | 40G |
| 4 concurrent | 2 | 4G | 20G |

## Examples

```bash
# Full development VM
./provision-vm.sh agent-01 \
  --profile agentic-dev \
  --agentshare \
  --cpus 8 \
  --memory 16G \
  --wait-ready

# Minimal test VM
./provision-vm.sh test-vm \
  --cpus 2 \
  --memory 2G \
  --start

# Specific IP
./provision-vm.sh agent-custom \
  --profile agentic-dev \
  --ip 192.168.122.100 \
  --start
```

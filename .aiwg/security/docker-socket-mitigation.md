# Docker Socket Privilege Escalation Mitigation Design

**Document Version**: 1.0
**Date**: 2026-01-31
**Classification**: Internal - Security Sensitive
**Author**: Security Architect
**Status**: Draft - Pending Architecture Review

---

## Executive Summary

The agentic-sandbox VMs currently grant agent users Docker group membership, which provides effective root access to the VM through the Docker socket. This document analyzes the threat, evaluates mitigation options, and recommends a phased approach to maintain container development capabilities while eliminating privilege escalation vectors.

**Key Finding**: Docker group membership = root equivalent access. An agent can escape any VM-level restrictions via:

```bash
docker run --rm -it --privileged --pid=host --net=host -v /:/host ubuntu chroot /host bash
```

**Recommended Solution**: Rootless Docker with seccomp restrictions (Phase 1) + Sandboxed Container API (Phase 2)

---

## 1. Threat Analysis

### 1.1 Current State

| Component | Configuration | Risk |
|-----------|---------------|------|
| Docker daemon | Root-mode (default) | HIGH - Socket grants root-equivalent access |
| Agent user | docker group member | HIGH - Full daemon control |
| Socket path | /var/run/docker.sock | Standard, no protection |
| Containers | No restrictions | CRITICAL - `--privileged` allowed |

### 1.2 Attack Vectors

#### Vector 1: Direct Host Filesystem Access

```bash
# Mount host root filesystem, chroot into it
docker run --rm -it -v /:/host ubuntu chroot /host bash
```

**Impact**: Read/write access to all VM files, including:
- `/etc/shadow` - password hashes
- `/etc/agentic-sandbox/agent.env` - agent secrets
- SSH authorized_keys - persistence

#### Vector 2: Privileged Container Escape

```bash
# Full privileged access with host PID/network namespace
docker run --rm -it --privileged --pid=host --net=host \
  -v /:/host ubuntu chroot /host bash
```

**Impact**: Complete VM compromise, access to host kernel features

#### Vector 3: Capabilities Abuse

```bash
# Gain CAP_SYS_ADMIN for mount namespace manipulation
docker run --rm -it --cap-add=SYS_ADMIN ubuntu bash
```

**Impact**: Mount manipulation, potential escape via cgroup release_agent

#### Vector 4: Device Access

```bash
# Direct access to block devices
docker run --rm -it --device=/dev/sda ubuntu dd if=/dev/sda
```

**Impact**: Raw disk access, data exfiltration, disk corruption

### 1.3 Why This Matters for Agentic-Sandbox

The security architecture document states:

> **Inside VM**: Agent has FULL control (sudo NOPASSWD, docker, filesystem)
> **Isolation**: Security comes from KVM hardware virtualization

This philosophy accepts VM-level root equivalent access. However:

1. **Audit trail bypass**: Container operations may not be logged
2. **Resource quota bypass**: Containers can exceed agent cgroup limits
3. **Network isolation bypass**: `--net=host` bypasses VM firewall rules
4. **Persistence mechanisms**: Agents can create rootkits via Docker

**Question**: Is full Docker access actually needed, or just container build/run capabilities?

---

## 2. Solution Comparison Matrix

### 2.1 Options Overview

| Solution | Privilege Level | Container UX | Build Support | Performance | Complexity |
|----------|-----------------|--------------|---------------|-------------|------------|
| **Rootless Docker** | Unprivileged | Native | Full | ~5% overhead | Medium |
| **Podman (rootless)** | Unprivileged | Docker-compatible | Full | ~3% overhead | Low |
| **gVisor (runsc)** | Sandboxed kernel | Native | Limited | ~20-50% overhead | High |
| **Kata Containers** | VM-in-VM | Native | Full | ~30% overhead | Very High |
| **Sandboxed API** | Proxy-controlled | Custom CLI | Allowlist | Minimal | Medium |
| **No Docker** | N/A | None | None | N/A | None |

### 2.2 Detailed Evaluation

#### Option A: Rootless Docker

**How it works**: Docker daemon runs as unprivileged user, using user namespaces for isolation. No socket privilege escalation possible.

**Pros**:
- Native Docker CLI compatibility
- Full build/compose/buildx support
- Agent cannot access host filesystem via containers
- Containers run as agent subUIDs (no root mapping)

**Cons**:
- Requires UID/GID subordinate range setup
- Some features limited (e.g., --privileged, --net=host)
- Overlay filesystem requires kernel support (Ubuntu 24.04: OK)
- Port binding below 1024 requires additional setup

**Security Posture**:
```
Before (root Docker):
  agent -> docker.sock -> dockerd (root) -> containers (root-capable)
                                         -> host mount possible

After (rootless Docker):
  agent -> docker.sock -> dockerd (agent) -> containers (agent subUIDs)
                                          -> no host access possible
```

**Implementation Effort**: Medium (2-3 days)

#### Option B: Podman (Rootless)

**How it works**: Daemonless container engine, runs entirely as user. OCI-compatible, Docker CLI alias available.

**Pros**:
- Daemonless architecture (no socket attack surface)
- Native rootless operation
- Docker CLI compatibility via `podman-docker` package
- Compose support via `podman-compose`
- Smaller attack surface than Docker

**Cons**:
- Docker Compose v2 features may differ
- Buildx not available (uses buildah instead)
- Some CI/CD tooling expects Docker specifically
- Minor CLI differences (podman-specific flags)

**Security Posture**:
```
agent -> podman (user process) -> containers (agent subUIDs)
                               -> no daemon, no socket privilege
```

**Implementation Effort**: Low (1-2 days)

#### Option C: gVisor (runsc)

**How it works**: User-space kernel intercepts syscalls, providing additional isolation layer. Works as OCI runtime with Docker/containerd.

**Pros**:
- Kernel vulnerability protection (syscalls handled in userspace)
- Works with existing Docker infrastructure
- Strong isolation for untrusted code

**Cons**:
- Significant performance overhead (20-50% for syscall-heavy workloads)
- Application compatibility issues (not all syscalls supported)
- Build operations may fail (filesystem syscall limitations)
- Debugging complexity

**Security Posture**:
- Does NOT prevent privilege escalation via Docker socket
- Provides additional isolation WITHIN containers
- Must be combined with rootless Docker or socket restrictions

**Implementation Effort**: High (1 week+ for compatibility testing)

#### Option D: Kata Containers

**How it works**: Each container runs in a lightweight VM, providing hardware isolation.

**Pros**:
- Strongest isolation (VM boundary per container)
- Kernel vulnerability protection
- Compatible with standard container tooling

**Cons**:
- Requires nested virtualization (VM-in-VM)
- 30%+ performance overhead
- Memory overhead per container
- Complex setup and maintenance
- May not work in all VM environments

**Security Posture**:
- Does NOT prevent privilege escalation via Docker socket
- Provides VM-level isolation for container workloads
- Overkill for this use case (already in VM)

**Implementation Effort**: Very High (2+ weeks)

#### Option E: Sandboxed Container API

**How it works**: Custom proxy intercepts container operations, enforcing policy before forwarding to Docker.

**Pros**:
- Fine-grained control over allowed operations
- Audit logging of all container actions
- Can allowlist specific images, capabilities, mounts
- Works with existing Docker installation

**Cons**:
- Custom development required
- Maintenance burden
- Potential bypass if agent discovers socket location
- CLI UX changes (unless transparent proxy)

**Architecture**:
```
agent -> container-proxy (host) -> policy engine -> docker.sock
                                                 -> allow/deny
```

**Implementation Effort**: Medium-High (1-2 weeks for basic implementation)

#### Option F: Remove Docker Access

**How it works**: Remove agent from docker group, disable Docker or run it root-only.

**Pros**:
- Complete elimination of attack vector
- Simplest implementation

**Cons**:
- Breaks container-based development workflows
- Agents cannot build/test containerized applications
- Significant capability reduction

**Implementation Effort**: Trivial (1 hour)

### 2.3 Recommendation Matrix

| Requirement | Rootless Docker | Podman | gVisor | Kata | Sandboxed API | No Docker |
|-------------|-----------------|--------|--------|------|---------------|-----------|
| Eliminate privilege escalation | YES | YES | NO | NO | PARTIAL | YES |
| Docker CLI compatibility | YES | MOSTLY | YES | YES | CUSTOM | NO |
| docker-compose support | YES | MOSTLY | YES | YES | LIMITED | NO |
| Multi-arch builds (buildx) | YES | NO | LIMITED | YES | POLICY | NO |
| Performance impact | LOW | LOW | HIGH | HIGH | LOW | N/A |
| Implementation complexity | MEDIUM | LOW | HIGH | VERY HIGH | MEDIUM | TRIVIAL |
| Ongoing maintenance | LOW | LOW | MEDIUM | HIGH | MEDIUM | NONE |

---

## 3. Recommended Approach

### 3.1 Strategy: Rootless Docker with Optional Sandboxed API

**Phase 1 (Short-term, 1-2 weeks)**: Migrate to Rootless Docker
**Phase 2 (Medium-term, 4-6 weeks)**: Implement Sandboxed Container API for enhanced control
**Phase 3 (Long-term)**: Evaluate gVisor for high-security workloads

### 3.2 Phase 1: Rootless Docker Implementation

#### 3.2.1 Installation Changes

**Current** (in `provision-vm.sh`):
```bash
apt-get install -y docker-ce docker-ce-cli containerd.io \
    docker-buildx-plugin docker-compose-plugin
usermod -aG docker "$TARGET_USER"
```

**Proposed**:
```bash
# Install Docker packages (daemon will be root, but not used by agent)
apt-get install -y docker-ce docker-ce-cli containerd.io \
    docker-buildx-plugin docker-compose-plugin uidmap dbus-user-session

# Do NOT add agent to docker group
# usermod -aG docker "$TARGET_USER"  # REMOVED

# Setup rootless Docker for agent user
sudo -u "$TARGET_USER" bash << 'ROOTLESS_EOF'
export HOME="/home/agent"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"

# Ensure runtime directory exists
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# Install rootless Docker
dockerd-rootless-setuptool.sh install

# Configure Docker CLI to use rootless socket
mkdir -p "$HOME/.docker"
cat > "$HOME/.docker/config.json" << 'DOCKER_CFG'
{
  "currentContext": "rootless"
}
DOCKER_CFG

# Enable rootless Docker daemon on login
systemctl --user enable docker
ROOTLESS_EOF

# Setup subordinate UID/GID ranges for agent user
echo "agent:100000:65536" >> /etc/subuid
echo "agent:100000:65536" >> /etc/subgid
```

#### 3.2.2 Runtime Configuration

**Rootless Docker Context** (`~/.docker/contexts/rootless/meta.json`):
```json
{
  "Name": "rootless",
  "Endpoints": {
    "docker": {
      "Host": "unix:///run/user/1000/docker.sock"
    }
  }
}
```

**Systemd User Service** (auto-created by setup tool):
```ini
[Unit]
Description=Docker Application Container Engine (Rootless)

[Service]
Environment=PATH=/usr/bin:/usr/local/bin
ExecStart=/usr/bin/dockerd-rootless.sh
Restart=always

[Install]
WantedBy=default.target
```

#### 3.2.3 Feature Restrictions

Rootless Docker inherently blocks dangerous operations:

| Operation | Root Docker | Rootless Docker | Reason |
|-----------|-------------|-----------------|--------|
| `--privileged` | Allowed | DENIED | No CAP_SYS_ADMIN |
| `--net=host` | Allowed | LIMITED | User namespace isolation |
| `-v /:/host` | Allowed | DENIED | No access to root filesystem |
| `--cap-add=SYS_ADMIN` | Allowed | DENIED | Not available in user namespace |
| `--device=/dev/sda` | Allowed | DENIED | No device access |
| Port < 1024 | Allowed | DENIED* | Requires sysctl or setcap |

*Can be enabled with: `sysctl net.ipv4.ip_unprivileged_port_start=0`

#### 3.2.4 Seccomp Profile for Rootless Docker

Create additional restrictions via seccomp profile:

**File**: `/etc/docker/seccomp-rootless.json`
```json
{
  "defaultAction": "SCMP_ACT_ALLOW",
  "syscalls": [
    {
      "names": [
        "mount",
        "umount2",
        "pivot_root",
        "unshare",
        "setns",
        "clone3"
      ],
      "action": "SCMP_ACT_ERRNO",
      "args": [],
      "comment": "Block namespace manipulation from within containers"
    },
    {
      "names": [
        "ptrace"
      ],
      "action": "SCMP_ACT_ERRNO",
      "args": [],
      "comment": "Block process debugging/tracing"
    }
  ]
}
```

**Apply via daemon config** (`~/.config/docker/daemon.json`):
```json
{
  "seccomp-profile": "/etc/docker/seccomp-rootless.json",
  "storage-driver": "overlay2",
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "10m",
    "max-file": "3"
  }
}
```

### 3.3 Phase 2: Sandboxed Container API (Optional Enhancement)

For environments requiring additional control, implement a proxy layer.

#### 3.3.1 Architecture

```
+---------------+          +------------------+          +---------------+
|   Agent       |  HTTP    |  Container Proxy |  Docker  |  Rootless     |
|   Process     |--------->|  (Rust/Go)       |--------->|  Docker       |
|               |          |                  |          |  Daemon       |
| docker build  |          | - Policy engine  |          |               |
| docker run    |          | - Audit logging  |          | unix://...    |
| docker push   |          | - Image allowlist|          |               |
+---------------+          +------------------+          +---------------+
```

#### 3.3.2 Policy Engine Rules

```yaml
# /etc/agentic-sandbox/container-policy.yaml
version: 1

defaults:
  allow_privileged: false
  allow_host_network: false
  allow_host_pid: false
  max_memory: "4g"
  max_cpus: 2
  allowed_capabilities: []

image_allowlist:
  - "ubuntu:*"
  - "debian:*"
  - "node:*"
  - "python:*"
  - "golang:*"
  - "rust:*"
  - "registry.internal/*"

mount_restrictions:
  denied_paths:
    - "/etc/shadow"
    - "/etc/sudoers"
    - "/root"
    - "/var/run/docker.sock"
  allowed_paths:
    - "/home/agent/workspace"
    - "/tmp"

audit:
  log_all_operations: true
  log_path: "/var/log/container-audit.log"
```

#### 3.3.3 Proxy Implementation Sketch

```rust
// container-proxy/src/main.rs
use axum::{Router, routing::post};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct CreateContainerRequest {
    image: String,
    cmd: Vec<String>,
    host_config: HostConfig,
}

#[derive(Deserialize)]
struct HostConfig {
    privileged: Option<bool>,
    network_mode: Option<String>,
    binds: Option<Vec<String>>,
    cap_add: Option<Vec<String>>,
}

async fn create_container(
    Json(req): Json<CreateContainerRequest>,
) -> Result<Json<ContainerResponse>, PolicyError> {
    // Enforce policy
    if req.host_config.privileged.unwrap_or(false) {
        return Err(PolicyError::Denied("privileged containers not allowed"));
    }

    if req.host_config.network_mode.as_deref() == Some("host") {
        return Err(PolicyError::Denied("host network not allowed"));
    }

    // Check image allowlist
    if !is_image_allowed(&req.image) {
        return Err(PolicyError::Denied("image not in allowlist"));
    }

    // Check mount paths
    for bind in req.host_config.binds.unwrap_or_default() {
        if is_sensitive_path(&bind) {
            return Err(PolicyError::Denied("mount path not allowed"));
        }
    }

    // Audit log
    audit_log(&req);

    // Forward to Docker daemon
    forward_to_docker(req).await
}
```

### 3.4 Phase 3: gVisor for High-Security Workloads

For tasks processing untrusted code (e.g., running user-submitted tests):

```bash
# Install gVisor runtime
wget https://storage.googleapis.com/gvisor/releases/release/latest/runsc
chmod +x runsc
mv runsc /usr/local/bin/

# Configure Docker to use gVisor
cat > ~/.config/docker/daemon.json << 'EOF'
{
  "runtimes": {
    "runsc": {
      "path": "/usr/local/bin/runsc",
      "runtimeArgs": [
        "--platform=ptrace"
      ]
    }
  }
}
EOF

# Run containers with gVisor
docker run --runtime=runsc -it python:3.12 python -c "print('sandboxed')"
```

---

## 4. Breaking Changes and Mitigations

### 4.1 Breaking Changes

| Change | Impact | Affected Workflows |
|--------|--------|-------------------|
| No docker group membership | Cannot access root Docker socket | Direct `docker` commands if rootless not configured |
| Rootless socket path | Socket at `~/run/docker.sock` instead of `/var/run/docker.sock` | Scripts hardcoding socket path |
| No `--privileged` | Cannot run privileged containers | DIND, kernel module testing |
| No `--net=host` | Cannot share host network | Network debugging, host service access |
| No host filesystem mounts | Cannot mount `/` or system paths | Host introspection, system utilities |
| Port < 1024 restrictions | Cannot bind to low ports directly | Web servers on 80/443 |

### 4.2 Mitigations

#### 4.2.1 Socket Path Compatibility

```bash
# Add to agent's .bashrc/.profile
export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/docker.sock"

# Or create symlink (requires root during setup)
ln -s "/run/user/$(id -u agent)/docker.sock" /var/run/docker.sock.agent
```

#### 4.2.2 Low Port Binding

```bash
# During VM provisioning (as root)
sysctl -w net.ipv4.ip_unprivileged_port_start=80
echo "net.ipv4.ip_unprivileged_port_start=80" >> /etc/sysctl.d/99-rootless-docker.conf
```

#### 4.2.3 Docker-in-Docker Alternative

For CI/CD workflows requiring DIND:

```bash
# Use Docker's official DIND approach with rootless
docker run --privileged docker:dind-rootless
```

Or use Sysbox (requires host kernel module):

```bash
# Sysbox enables unprivileged DIND
docker run --runtime=sysbox-runc -it docker:dind
```

#### 4.2.4 Network Debugging

```bash
# Instead of --net=host, use port forwarding
docker run -p 8080:80 nginx

# For debugging, use network tools inside container
docker run --rm -it nicolaka/netshoot
```

### 4.3 Compatibility Matrix

| Use Case | Root Docker | Rootless Docker | Notes |
|----------|-------------|-----------------|-------|
| Build images | YES | YES | No change |
| Run containers | YES | YES | No change |
| Multi-arch builds | YES | YES | buildx works |
| docker-compose | YES | YES | No change |
| Mount /workspace | YES | YES | User paths OK |
| Mount / (root) | YES | NO | Blocked (by design) |
| --privileged | YES | NO | Blocked (by design) |
| --net=host | YES | LIMITED | Port mapping alternative |
| --device | YES | NO | Blocked (by design) |
| Kubernetes-in-Docker | YES | PARTIAL | Use Sysbox or k3d |

---

## 5. Implementation Plan

### 5.1 Phase 1: Rootless Docker (Week 1-2)

**Day 1-2**: Development and Testing
- [ ] Modify `provision-vm.sh` to install rootless Docker
- [ ] Create test VM with new configuration
- [ ] Verify basic Docker operations work
- [ ] Document any compatibility issues

**Day 3-4**: Compatibility Testing
- [ ] Test docker-compose workflows
- [ ] Test multi-stage builds
- [ ] Test buildx cross-compilation
- [ ] Test common development images (Node, Python, Go, Rust)

**Day 5-7**: Integration and Documentation
- [ ] Update base image build scripts
- [ ] Update ENVIRONMENT.md generated in VMs
- [ ] Create migration guide for existing workflows
- [ ] Update security documentation

### 5.2 Phase 2: Sandboxed API (Week 3-6)

**Week 3**: Design and Scaffolding
- [ ] Define policy schema
- [ ] Implement proxy skeleton (Rust)
- [ ] Add Docker API forwarding

**Week 4**: Policy Engine
- [ ] Implement image allowlist
- [ ] Implement mount restrictions
- [ ] Implement capability restrictions
- [ ] Add audit logging

**Week 5-6**: Integration and Testing
- [ ] Integrate with agent-client
- [ ] End-to-end testing
- [ ] Performance benchmarking
- [ ] Security review

### 5.3 Phase 3: gVisor Evaluation (Month 2-3)

- [ ] Benchmark gVisor overhead for typical workloads
- [ ] Test application compatibility
- [ ] Define criteria for "high-security" tasks
- [ ] Create optional gVisor runtime profile

---

## 6. Security Model Documentation

### 6.1 Before (Current State)

```
Threat Surface:
  - Docker socket grants root-equivalent access
  - Agent can escape all VM-level restrictions via Docker
  - No audit trail for container operations
  - Resource limits bypassable via container configuration

Trust Model:
  - Relies entirely on KVM boundary for isolation
  - Agent has effective root in VM
  - Container operations are uncontrolled

Attack Path:
  agent -> docker run --privileged -> host filesystem access -> secret extraction
```

### 6.2 After (Rootless Docker)

```
Threat Surface:
  - Docker socket grants agent-level access only
  - Agent cannot escape user namespace via Docker
  - Container operations logged
  - Resource limits enforced by user namespace

Trust Model:
  - KVM boundary provides host isolation
  - User namespace provides VM-internal isolation
  - Dangerous Docker flags blocked by architecture

Remaining Risks:
  - Container escapes via kernel vulnerability (mitigated by user namespace)
  - Resource exhaustion within agent limits (acceptable)
  - Network access from containers (controlled by VM firewall)
```

### 6.3 Security Guarantees

| Guarantee | How Enforced |
|-----------|--------------|
| Agent cannot access host filesystem | Rootless Docker user namespace |
| Agent cannot gain root in VM via Docker | No docker group, no root socket access |
| Container network isolated | VM-level UFW rules apply |
| Container resource usage limited | Agent cgroup limits inherited |
| Container operations audited | Docker daemon logging |

---

## 7. Verification Checklist

### 7.1 Pre-Deployment Gate

- [ ] `docker run --privileged` fails with "permission denied"
- [ ] `docker run -v /:/host` fails with "permission denied"
- [ ] `docker run --net=host` fails or is limited to user namespace
- [ ] Agent cannot read `/var/run/docker.sock` (root socket)
- [ ] Rootless socket at expected path and working
- [ ] Docker Compose operations work
- [ ] Multi-stage builds work
- [ ] Buildx cross-compilation works

### 7.2 Security Tests

```bash
# Test 1: Verify privileged fails
docker run --rm --privileged ubuntu id
# Expected: Error - cannot use privileged in rootless mode

# Test 2: Verify host mount fails
docker run --rm -v /:/host ubuntu ls /host
# Expected: Error - permission denied

# Test 3: Verify host network is limited
docker run --rm --net=host ubuntu ip addr
# Expected: Limited or error

# Test 4: Verify device access fails
docker run --rm --device=/dev/sda ubuntu ls /dev/sda
# Expected: Error - no such device

# Test 5: Verify root socket inaccessible
ls -la /var/run/docker.sock
# Expected: Agent cannot read

# Test 6: Verify normal operations work
docker run --rm ubuntu echo "hello"
# Expected: Success
```

---

## 8. References

| Document | Path |
|----------|------|
| STRIDE Threat Model | `/home/roctinam/dev/agentic-sandbox/.aiwg/security/threat-model.md` |
| Security Architecture | `/home/roctinam/dev/agentic-sandbox/.aiwg/security/security-architecture.md` |
| VM Provisioning Script | `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh` |
| Docker Hardening Spike | `/home/roctinam/dev/agentic-sandbox/.aiwg/spikes/spike-002-docker-hardening.md` |
| Docker Rootless Mode | https://docs.docker.com/engine/security/rootless/ |
| Podman Documentation | https://podman.io/docs |
| gVisor Documentation | https://gvisor.dev/docs/ |

---

## 9. Appendix A: Rootless Docker Installation Script

Complete installation script for Phase 1:

```bash
#!/bin/bash
# install-rootless-docker.sh
# Run during VM provisioning to setup rootless Docker

set -euo pipefail

TARGET_USER="${1:-agent}"
USER_HOME="/home/$TARGET_USER"
USER_ID=$(id -u "$TARGET_USER")

log() { echo "[rootless-docker] $1"; }

# Prerequisites
log "Installing prerequisites..."
apt-get update
apt-get install -y uidmap dbus-user-session slirp4netns

# Setup subordinate UID/GID ranges
log "Configuring subuid/subgid..."
if ! grep -q "^$TARGET_USER:" /etc/subuid; then
    echo "$TARGET_USER:100000:65536" >> /etc/subuid
fi
if ! grep -q "^$TARGET_USER:" /etc/subgid; then
    echo "$TARGET_USER:100000:65536" >> /etc/subgid
fi

# Install Docker CE (root daemon for system services if needed)
log "Installing Docker CE..."
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] \
    https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" \
    > /etc/apt/sources.list.d/docker.list
apt-get update
apt-get install -y docker-ce docker-ce-cli containerd.io \
    docker-buildx-plugin docker-compose-plugin

# DO NOT add user to docker group (this is the key security change)
# usermod -aG docker "$TARGET_USER"  # INTENTIONALLY OMITTED

# Stop root Docker daemon if not needed
# systemctl stop docker
# systemctl disable docker

# Enable lingering for user (allows user services without login)
loginctl enable-linger "$TARGET_USER"

# Create XDG_RUNTIME_DIR
mkdir -p "/run/user/$USER_ID"
chown "$TARGET_USER:$TARGET_USER" "/run/user/$USER_ID"
chmod 700 "/run/user/$USER_ID"

# Setup rootless Docker as agent user
log "Installing rootless Docker for $TARGET_USER..."
sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$USER_ID" bash << 'ROOTLESS_EOF'
export HOME="/home/agent"
export PATH="$HOME/.local/bin:$PATH"

# Run rootless setup
dockerd-rootless-setuptool.sh install

# Create Docker config
mkdir -p "$HOME/.docker"
cat > "$HOME/.docker/config.json" << 'DOCKER_CFG'
{
  "currentContext": "rootless"
}
DOCKER_CFG

# Enable service
systemctl --user enable docker
systemctl --user start docker
ROOTLESS_EOF

# Configure low port binding (optional)
log "Configuring low port binding..."
echo "net.ipv4.ip_unprivileged_port_start=80" > /etc/sysctl.d/99-rootless-docker.conf
sysctl -p /etc/sysctl.d/99-rootless-docker.conf

# Add environment setup to profile
cat >> "$USER_HOME/.bashrc" << 'BASHRC_EOF'

# Rootless Docker configuration
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/docker.sock"
export PATH="$HOME/.local/bin:$PATH"
BASHRC_EOF

log "Rootless Docker installation complete"
log "User '$TARGET_USER' can now use Docker without root privileges"
log "Dangerous flags (--privileged, -v /:/, etc.) are blocked by design"
```

---

## 10. Appendix B: Podman Alternative Installation

If Podman is preferred over Docker:

```bash
#!/bin/bash
# install-podman.sh

set -euo pipefail

TARGET_USER="${1:-agent}"

log() { echo "[podman] $1"; }

# Install Podman
log "Installing Podman..."
apt-get update
apt-get install -y podman podman-compose buildah skopeo

# Setup subordinate UID/GID
echo "$TARGET_USER:100000:65536" >> /etc/subuid
echo "$TARGET_USER:100000:65536" >> /etc/subgid

# Create Docker CLI alias
cat > /usr/local/bin/docker << 'EOF'
#!/bin/bash
exec podman "$@"
EOF
chmod +x /usr/local/bin/docker

# Configure registries
mkdir -p /etc/containers
cat > /etc/containers/registries.conf << 'EOF'
[registries.search]
registries = ['docker.io', 'quay.io', 'ghcr.io']

[registries.insecure]
registries = []

[registries.block]
registries = []
EOF

log "Podman installation complete"
log "Docker CLI aliased to Podman for compatibility"
```

---

## 11. Document Approval

| Role | Name | Date | Status |
|------|------|------|--------|
| Author | Security Architect | 2026-01-31 | Complete |
| Reviewer | Principal Architect | Pending | - |
| Reviewer | DevOps Lead | Pending | - |
| Approver | Project Owner | Pending | - |

---

## Revision History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-01-31 | Security Architect | Initial design document |

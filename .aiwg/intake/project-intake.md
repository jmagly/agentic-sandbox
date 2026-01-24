# Project Intake Form

**Document Type**: Existing System Enhancement (Early Implementation)
**Generated**: 2026-01-05
**Source**: Codebase analysis + user requirements validation

## Metadata

- **Project name**: Agentic Sandbox
- **Requestor/owner**: IntegRO Labs / roctinam
- **Date**: 2026-01-05
- **Stakeholders**: Engineering (primary), Security/Compliance, Operations/SRE

## System Overview

**Purpose**: Runtime isolation tooling for persistent, unrestricted agent processes. Provides preconfigured Docker containers and QEMU VMs for agentic workloads with secure isolation from host systems, credential proxy injection, and controlled external system access.

**Current Status**: Early Implementation (Docker runtime functional with security hardening, QEMU structure defined, integration bridges planned)

**Users**: Small internal team (2-10 developers) at IntegRO Labs, running 5-10 concurrent agent sandboxes for autonomous development tasks

**Tech Stack** (current implementation):
- **Languages**: Bash (orchestration), YAML (configuration)
- **Container Runtime**: Docker with security hardening (seccomp, capability dropping, isolated networks)
- **VM Runtime**: QEMU/KVM with libvirt orchestration
- **Base Images**: Ubuntu 24.04 LTS
- **Agent Tooling**: Claude Code CLI, Node.js 22, Python 3, build-essential
- **Orchestration**: Bash launch scripts, docker-compose configurations, libvirt XML definitions
- **Security**: seccomp syscall filtering, Linux capabilities restriction, network isolation

## Problem and Outcomes

**Problem Statement**:
Current agentic workflows lack proper runtime isolation, creating security risks when agents handle code repositories, production data, and sensitive credentials. Agents running on developer workstations have excessive host access, credential exposure risks, and no enforced resource limits. Need secure, isolated environments where agents can run autonomously for hours/days without compromising host security or leaking credentials.

**Target Personas**:
- Primary: Development team members launching autonomous coding agents for complex multi-hour tasks (refactoring, migrations, testing)
- Secondary: Operations team running agent-based automation with production system access
- Future: Security researchers analyzing untrusted agent code in air-gapped VMs

**Success Metrics (KPIs)**:
- **Security validation**: Agents cannot escape sandbox isolation (verified via security testing), credential proxy prevents secret exposure (100% of credentials injected, 0% stored in containers)
- **Developer productivity**: Team actively uses sandboxes for 80%+ of long-running agent tasks, replaces ad-hoc agent execution
- **Task automation**: Agents complete 10+ autonomous tasks/week with multi-hour runtimes, demonstrating long-lived capability
- **Performance**: Container launch <30s, VM launch <2min, resource isolation allows 5-10 concurrent sandboxes on single host
- **Learning/experimentation**: Proven architecture for future production agentic workloads, validated security model

## Current Scope and Features

**Core Features** (in-scope for current phase):

**Docker Runtime (Implemented)**:
- Launch isolated Docker containers with security hardening (seccomp, capabilities, network isolation)
- Resource limits (CPU, memory) enforced per container
- Volume mounts for workspace persistence
- Environment variable injection (API keys, configuration)
- Detached (background) and interactive modes
- Health checks for agent processes
- Structured logging (JSON, retention limits)

**QEMU Runtime (Partially Implemented)**:
- Launch full VMs via libvirt for maximum isolation
- Resource configuration (memory, CPU, disk)
- Persistent workspace disks separate from system disk
- Serial console access
- GPU passthrough support (configured, not tested)
- VM lifecycle management (start, stop, destroy)

**Security Hardening (Implemented for Docker, Planned for QEMU)**:
- seccomp syscall filtering (comprehensive allow-list)
- Linux capability dropping (drop ALL, add minimal necessary)
- Network isolation (internal bridge, no external access by default)
- Read-only root filesystem option
- No privileged containers
- Audit logging

**Agent Definitions (Schema Defined, Runtime Support Partial)**:
- YAML-based agent configuration
- Resource allocation (CPU, memory, disk, timeout)
- Volume mount specifications
- Environment variables
- Integration bridge configuration (git, s3, container registry, databases)
- Security settings (network mode, capabilities, read-only root)
- Lifecycle hooks (pre/post start/stop)
- Health check definitions

**Credential Proxy Model (Planned, Critical)**:
- Inject pre-authenticated access proxies into containers
- Git SSH/HTTPS proxy (agent clones/pushes without seeing keys)
- Cloud storage proxy (S3-compatible, credentials external)
- Container registry proxy (Docker socket proxy)
- Database network proxy (credentials never enter container)
- External API proxies for authenticated services
- Secrets mounted via Docker secrets, never in environment variables

**Out-of-Scope** (explicitly deferred):

**Integration Bridges Implementation**:
- Git proxy server (agent YAML defines intent, implementation deferred)
- S3-compatible storage bridge
- Container registry proxy
- Database connection proxy
- Generic API proxy framework
- Rationale: Core isolation must be proven secure first, then add controlled external access

**Multi-Host Orchestration**:
- Kubernetes operator for cluster-wide agent scheduling
- Multi-host VM orchestration
- Cross-host networking
- Rationale: Validate single-host model before scaling complexity

**Web UI**:
- Browser-based sandbox management
- Real-time agent monitoring dashboards
- Log aggregation and search
- Rationale: CLI-first for power users, Web UI adds complexity without security validation

**Advanced Features**:
- Checkpoint/resume for long-running agents
- Live migration between hosts
- Nested virtualization for agent-in-agent
- Windows VM support
- Rationale: Focus on core security and Linux/Docker foundation

**Future Considerations** (post-validation):
- **Integration bridge production implementation**: Once core isolation proven, implement git/S3/database proxies with credential injection model
- **Multi-host orchestration**: Kubernetes operator for production-scale agent scheduling
- **Web UI for management**: Browser-based interface for non-technical users
- **Advanced VM features**: Checkpoint/resume, live migration, GPU-accelerated workloads at scale
- **Enterprise features**: RBAC, audit trail compliance, SOC2 controls for regulated environments

## Architecture (Current Implementation)

**Architecture Style**: Hybrid isolation - Docker containers for fast iteration + QEMU VMs for maximum security

**Chosen**: **Hybrid (Container + VM)** - **Rationale**:
- Docker provides fast launch (<30s), low overhead, suitable for trusted/semi-trusted agent code with strong kernel isolation
- QEMU provides hardware-level isolation for untrusted workloads, GPU passthrough, full OS control
- Small team benefits from flexibility: quick Docker iteration for development, VM fallback for security-critical work
- Both runtimes share common agent definition schema, credential proxy model

**Current Implementation Components**:

**1. Sandbox Launcher (scripts/sandbox-launch.sh)**:
- Unified CLI for both Docker and QEMU runtimes
- Argument parsing: runtime, image, resources, mounts, environment
- Docker: Executes `docker run` with security hardening flags (seccomp, capabilities, network isolation)
- QEMU: Generates libvirt XML from template, adjusts resources, launches via `virsh`
- Supports interactive and detached (background) modes

**2. Base Container Image (images/base/Dockerfile)**:
- Ubuntu 24.04 LTS foundation
- Minimal tooling: git, curl, wget, ssh-client, jq, sudo
- Non-root `agent` user with sudo access
- `/workspace` directory for persistent work
- `/opt/sandbox` for runtime scripts

**3. Claude Agent Image (images/agent/claude/)**:
- Extends base image with development tools (Node.js 22, Python 3, build-essential, ripgrep, tmux)
- Claude Code CLI installed globally
- Custom entrypoint.sh for initialization (git config, SSH key setup, API key validation)
- Supports autonomous mode (timeout-enforced task execution) and interactive mode

**4. Docker Compose Configuration (runtimes/docker/docker-compose.yml)**:
- Security hardening: seccomp profile, capability dropping, no-new-privileges
- Resource limits (4 CPU, 8GB memory)
- Isolated network bridge (no external access by default)
- Volume mounts for workspace and cache persistence
- Docker secrets integration (git-credentials, ssh-key from files)
- Health checks via process monitoring
- Structured JSON logging with rotation

**5. QEMU VM Definition (runtimes/qemu/ubuntu-agent.xml)**:
- KVM-accelerated Ubuntu VM with UEFI boot
- Configurable resources (default 8GB RAM, 4 vCPU)
- Two disk images: system disk (qcow2) + workspace disk (persistent)
- VirtIO drivers for performance (network, disk, RNG)
- Isolated network bridge (matches Docker network model)
- Serial console for access
- GPU passthrough configuration (commented, ready for enablement)
- Memory balloon for dynamic resource adjustment

**6. Security Configuration (configs/seccomp-profile.json)**:
- Comprehensive syscall allow-list (default deny, explicit allow)
- Permits: file I/O, networking, process management, memory ops, time, signals
- Blocks: kernel module loading, system reboot, privileged operations
- 200+ allowed syscalls for full development environment compatibility

**7. Agent Definition Schema (agents/example-agent.yaml)**:
- Declarative YAML for agent configuration
- Resource allocation (CPU, memory, disk, timeout)
- Volume mounts with source/target/mode
- Environment variables
- Integration specifications (git, s3, container registry)
- Security overrides (network mode, capabilities, read-only root)
- Health check commands
- Lifecycle hooks for pre/post start/stop automation

**Data Models** (current):

**Agent Definition**:
```yaml
name: string                  # Unique identifier
runtime: docker|qemu          # Execution environment
image: string                 # Base image name
resources:
  cpu: integer                # CPU allocation
  memory: string              # Memory limit (e.g., "8G")
  disk: string                # Disk size (QEMU only)
  timeout: integer            # Max runtime (seconds)
mounts:
  - source: string            # Host path
    target: string            # Container/VM path
    mode: ro|rw               # Read-only or read-write
environment:
  KEY: value                  # Environment variables
integrations:
  git|s3|registry:            # External system bridges
    enabled: boolean
    configuration: map
security:
  network: isolated|bridged|host
  read_only_root: boolean
  privileged: boolean
  capabilities:
    drop: [ALL]
    add: [NET_BIND_SERVICE, ...]
```

**Integration Points** (planned, not implemented):

**Git Proxy**:
- HTTPS/SSH bridge service running on host
- Agent configures git remote pointing to proxy
- Proxy authenticates to actual git server using host credentials
- Agent never sees SSH keys or HTTPS tokens

**S3 Proxy**:
- S3-compatible API endpoint (MinIO-style)
- Agent uses standard S3 SDKs pointing to proxy
- Proxy forwards to real S3 with host credentials
- Supports bucket isolation per agent

**Container Registry Proxy**:
- Docker socket proxy for registry operations
- Agent can push/pull images via proxy
- Registry credentials managed on host, injected via proxy

**Database Proxy**:
- TCP proxy for PostgreSQL/MySQL/MongoDB
- Agent connects to localhost:port inside container
- Proxy forwards to real database with authentication
- No database credentials in container environment

**API Proxy (Generic)**:
- HTTP(S) proxy for external APIs
- Bearer tokens, API keys managed on host
- Agent makes requests via proxy with automatic auth injection

## Scale and Performance (Current Targets)

**Target Capacity**:
- **Concurrent sandboxes**: 5-10 Docker containers or 2-3 QEMU VMs on single host (assumed 32-64GB RAM, 16+ CPU workstation)
- **Team size**: 2-10 developers actively launching sandboxes
- **Usage pattern**: Mixed - some short-lived tasks (minutes), most long-lived (hours to days)

**Performance Targets**:
- **Docker launch latency**: <30 seconds from command to agent ready
- **QEMU launch latency**: <2 minutes from command to VM console available
- **Resource isolation**: No noticeable host performance degradation with 5 concurrent Docker sandboxes
- **Workspace persistence**: Survive container/VM restarts, data retained for days/weeks
- **Network throughput**: Git clone, package downloads at host network speeds (100+ Mbps)

**Performance Strategy**:
- **Layered container images**: Separate base image (rarely changes) from agent-specific layers (frequent updates) for fast rebuilds
- **qcow2 thin provisioning**: VM disks allocated on-demand, not upfront, saving host storage
- **VirtIO drivers**: Paravirtualized I/O for near-native VM performance
- **CPU pinning (future)**: Dedicate host cores to VM vCPUs for consistent performance
- **Image caching**: Pre-pull container images, pre-build VM templates to avoid cold-start delays

## Security and Compliance (Requirements)

**Security Posture**: **Strong** (threat model documented, proactive security testing, defense-in-depth isolation)

**Rationale**:
- Agents handle code repositories, production data, customer information
- Credential proxy model requires trust boundary enforcement
- Untrusted agent code scenarios (third-party agents, experimental AI models)
- 30+ year security expertise on team enables proper implementation

**Data Classification**:

**Handled Data Types**:
- **Code repositories** (Confidential): Proprietary source code, intellectual property
- **Customer/production data** (Restricted): PII, production database access via proxies
- **Sensitive credentials** (Restricted): API keys, SSH keys, cloud credentials (proxy-injected, never stored in containers)
- **Internal/test data** (Internal): Development datasets, synthetic data, test fixtures

**Classification**: **Restricted** (highest sensitivity due to production access and credentials)

**Security Controls** (current implementation):

**Container Isolation (Docker)**:
- Seccomp syscall filtering (200+ allowed, dangerous syscalls blocked)
- Linux capability dropping (ALL dropped, minimal re-added: NET_BIND_SERVICE, CHOWN, SETUID, SETGID)
- Network isolation (internal bridge, no external access by default, explicit egress required)
- Read-only root filesystem option (disabled by default for flexibility, enable for production)
- No privileged containers (enforced)
- Resource limits (CPU, memory) via cgroups
- Non-root user execution (agent user, UID 1000)

**VM Isolation (QEMU)**:
- Hardware-level isolation via KVM hypervisor
- Separate kernel, no shared host resources
- VirtIO drivers for I/O (performance + isolation)
- No host filesystem access without explicit mount
- Isolated network bridge (same model as Docker)
- UEFI secure boot ready (OVMF loader configured)

**Credential Management (Planned - Critical Gap)**:
- Docker secrets for git credentials, SSH keys (file-based, mounted at /run/secrets/)
- Environment variables for non-sensitive config only (never for credentials)
- Proxy injection model: credentials never enter container environment or filesystem
- Secrets rotation: host-side credential updates, no container rebuild required

**Audit Logging**:
- Structured JSON logs from containers (docker logging driver)
- 50MB max log size, 3 file rotation (prevents disk exhaustion)
- QEMU console logging via libvirt
- Lifecycle events logged: start, stop, task completion, errors

**Security Testing (Planned)**:
- Container escape attempts (seccomp bypass, capability exploitation)
- Credential leakage tests (verify proxy model, no secrets in container)
- Network isolation validation (blocked egress, internal-only communication)
- Resource exhaustion resistance (CPU, memory, disk bombs)
- QEMU VM breakout attempts (virtio vulnerabilities, hypercall exploits)

**Compliance Requirements**:

**Current**: None (internal tool, no regulatory mandate)

**Future Considerations**:
- **SOC2** (if customer sandboxes): Audit logging, access controls, incident response
- **GDPR** (if EU customer data): Data deletion capability, access logs, privacy controls
- **ISO27001** (if enterprise sales): Information security management system, risk assessments

## Team and Operations (Current)

**Team Size**: 2-10 developers (small, expert team)

**Team Skills**:
- **Principal Architect** (30+ years): Deep Linux security (seccomp, capabilities, namespaces), Docker/QEMU/KVM expertise, cloud infrastructure (AWS/GCP/Azure)
- **Development Team**: Full-stack capable, infrastructure-aware, comfortable with CLI tools
- **DevOps Experience**: Strong - Docker, libvirt, bash scripting, security hardening

**Development Velocity**:
- **Sprint length**: No formal sprints, continuous iteration driven by experimentation
- **Release frequency**: Ongoing development, no fixed releases, feature-driven milestones
- **Timeline**: Open-ended, phased approach (validate Docker security → implement QEMU fully → add integration bridges)

**Process Maturity** (planned):

**Version Control**:
- Git with conventional commits (`type(scope): subject`)
- Feature branches for experiments
- Direct-to-main for small changes (expert team, low coordination overhead)

**Code Review**:
- Peer review for security-critical changes (seccomp profiles, credential handling)
- Solo commits acceptable for documentation, configuration tweaks (trusted team)

**Testing**:
- **Target coverage**: 30-40% (security-critical paths, integration tests for launch scripts)
- **Manual testing**: Security validation (escape attempts, credential leakage checks)
- **Integration tests**: Docker/QEMU launch, resource limits, network isolation
- No unit tests for bash scripts initially (complexity vs. benefit trade-off)

**CI/CD**:
- Git hooks for commit linting (conventional commits)
- Manual builds initially (fast iteration, small team)
- Future: GitHub Actions for container image builds, security scanning (trivy, grype)

**Documentation**:
- README (comprehensive, usage examples)
- CLAUDE.md (project context for AI agents)
- Inline comments in security-critical code (seccomp, capabilities)
- No formal architecture docs yet (intake forms provide structure)

**Operational Support**:

**Monitoring**:
- Logs: `docker logs`, `virsh console` for manual inspection
- Metrics: None yet (future: resource usage, sandbox count, task completion)
- Alerting: None (internal tool, best-effort availability)

**Logging**:
- Docker: JSON logs with rotation (50MB, 3 files)
- QEMU: libvirt console logs
- Host: Bash script output to stdout/stderr

**On-Call**:
- None (internal tool, no SLA, business hours support)

**Incident Response**:
- Best-effort troubleshooting
- Security incidents: Immediate triage (container escape, credential leak)

## Dependencies and Infrastructure

**Third-Party Services** (current and planned):

**Required**:
- **Anthropic API**: Claude Code CLI requires API key (injected via environment variable)
- **Git hosting** (GitHub, GitLab, Gitea): Agents clone/push repos via proxy (planned)
- **Container registry** (Docker Hub, GitHub Packages): Base image distribution, agent image caching

**Planned**:
- **Cloud storage** (AWS S3, MinIO): Artifact persistence, workspace backups
- **Monitoring/Logging** (future): Datadog, Grafana, ELK stack for production observability

**Infrastructure** (current):

**Hosting**:
- Developer workstations (32-64GB RAM, 16+ CPU, NVMe SSD, NVIDIA GPU optional)
- No cloud deployment yet (local-first development)

**Runtime**:
- Docker Engine 24+ (required for security features)
- QEMU 8.0+ with KVM support (hardware virtualization)
- libvirt 9.0+ (QEMU orchestration)

**Storage**:
- Local SSD for container images, VM disk images (fast I/O)
- qcow2 thin provisioning for VM disks (efficient space usage)

**Networking**:
- Docker bridge networks (isolated, internal-only by default)
- libvirt virtual networks (same isolation model as Docker)
- No public IP assignment to sandboxes

## Known Risks and Uncertainties

**Technical Risks**:

**1. Container Escape Vulnerabilities** (HIGH impact, MEDIUM likelihood):
- **Description**: Despite seccomp + capabilities, kernel vulnerabilities could allow container breakout
- **Impact**: Agent gains host access, credential theft, privilege escalation
- **Mitigation**:
  - Regular security testing (escape attempts, exploit PoCs)
  - Kernel updates (track CVEs, patch quickly)
  - QEMU fallback for highest-risk workloads (hardware isolation)
  - Credential proxy model (even if escaped, no credentials in container)
- **Reassessment**: Quarterly security review, after major kernel updates

**2. Credential Proxy Implementation Complexity** (MEDIUM impact, HIGH likelihood):
- **Description**: Git/S3/database proxies require careful design to avoid credential leakage
- **Impact**: Implementation delays, potential security gaps if done wrong
- **Mitigation**:
  - Prototype git proxy first (most common use case)
  - Security review of proxy code before production use
  - Start with read-only access (clone repos) before write (push)
  - Comprehensive testing: network sniffing, container inspection for credential artifacts
- **Reassessment**: After first proxy implementation (git), validate model before expanding

**3. QEMU Performance Overhead** (MEDIUM impact, MEDIUM likelihood):
- **Description**: VM overhead may make long-running agents impractical (slower than containers)
- **Impact**: Users avoid QEMU, rely solely on Docker (reduces security posture for untrusted workloads)
- **Mitigation**:
  - VirtIO paravirtualization (already configured)
  - CPU pinning for dedicated cores
  - GPU passthrough for compute-heavy tasks
  - Benchmark: Docker vs QEMU for realistic agent workloads
- **Reassessment**: After first QEMU production usage (measure actual overhead)

**4. Resource Exhaustion (Fork Bombs, Disk Fills)** (LOW impact, MEDIUM likelihood):
- **Description**: Malicious or buggy agent code could exhaust host resources
- **Impact**: Host instability, impacts other sandboxes, manual cleanup required
- **Mitigation**:
  - cgroups limits (CPU, memory already configured)
  - Add: PID limits (prevent fork bombs), disk quotas (prevent disk exhaustion)
  - Monitoring: Resource usage alerts (future)
  - Emergency shutdown: Kill runaway sandboxes without data loss
- **Reassessment**: After first resource exhaustion incident

**Integration Risks**:

**1. API Rate Limits (Anthropic, GitHub, etc.)** (MEDIUM impact, MEDIUM likelihood):
- **Description**: Concurrent agents may hit API rate limits, causing task failures
- **Impact**: Agent tasks fail, user frustration, require manual retries
- **Mitigation**:
  - Rate limit awareness in agent code
  - Retry logic with exponential backoff
  - Monitor API usage, adjust concurrent sandbox count
  - Cache API responses where possible (git repo metadata, package indexes)
- **Reassessment**: Monthly usage review, adjust limits proactively

**2. Proxy Service Single Point of Failure** (MEDIUM impact, LOW likelihood):
- **Description**: If git/S3 proxy crashes, all agents lose external access
- **Impact**: Agent tasks block, no progress until proxy restored
- **Mitigation**:
  - Lightweight proxy design (minimal failure modes)
  - Systemd service for auto-restart
  - Health checks, alerting on proxy failure
  - Graceful degradation: agents queue operations, retry when proxy restored
- **Reassessment**: After proxy production deployment

**Timeline Risks**:

**1. Scope Creep (Web UI, Multi-Host, Advanced Features)** (MEDIUM impact, HIGH likelihood):
- **Description**: Feature requests expand scope beyond core isolation validation
- **Impact**: Delays security validation, team distraction, unfinished foundation
- **Mitigation**:
  - Explicit out-of-scope list (intake form documents deferral rationale)
  - Focus on Docker security validation + credential proxy MVP first
  - Defer all "nice-to-have" features until core proven
  - Regular scope review: is this necessary for security validation?
- **Reassessment**: Monthly scope check, ruthlessly defer non-essential work

**Team Risks**:

**1. Expertise Dependency (Principal Architect as SPOF)** (HIGH impact, LOW likelihood):
- **Description**: Deep security expertise concentrated in one person
- **Impact**: Delays if unavailable, knowledge gaps for rest of team
- **Mitigation**:
  - Documentation: Architecture decisions, security rationale in ADRs
  - Pairing: Junior team members shadow security work
  - Code comments: Explain non-obvious security logic (seccomp, capabilities)
  - Gradual knowledge transfer: Team members own subcomponents after training
- **Reassessment**: Quarterly knowledge sharing sessions

## Why This Intake Now?

**Context**:
Early implementation from rough specification (directory structure created, basic Docker runtime functional, QEMU scaffolding present). Need to validate what exists, identify gaps, and chart path to complete, production-ready system before expanding usage beyond initial experimentation.

**Goals**:
- **Validate current implementation**: Assess Docker security hardening completeness, QEMU readiness, identify missing pieces
- **Gap analysis**: Document delta between current state and production requirements (credential proxy, integration bridges, testing)
- **Establish requirements baseline**: Capture security posture, scale targets, compliance needs before scaling usage
- **Enable structured SDLC**: Apply Inception → Elaboration → Construction framework to systematically address gaps
- **Team alignment**: Principal architect expertise + team implementation, shared understanding of security priorities
- **Risk identification**: Surface security risks early (container escape, credential leakage) for proactive mitigation

**Triggers**:
- **Initial implementation complete**: Docker runtime works, QEMU defined, ready for gap assessment
- **Security validation needed**: Before expanding usage, must prove isolation guarantees hold
- **Team coordination**: Moving from solo experimentation to team adoption requires shared requirements
- **Guidance provided**: "Brand new from rough spec, need to validate and prepare for full completion"

## Attachments

- Solution profile: `.aiwg/intake/solution-profile.md`
- Option matrix: `.aiwg/intake/option-matrix.md`
- Architecture diagram: README.md (existing)
- QEMU VM definition: `runtimes/qemu/ubuntu-agent.xml`
- seccomp profile: `configs/seccomp-profile.json`
- Example agent definition: `agents/example-agent.yaml`

## Next Steps

**Immediate Actions** (Inception Phase):

1. **Review and validate intake documents**: Confirm requirements accuracy with team, adjust security posture or scope if needed

2. **Gap analysis deep-dive**:
   - Security testing: Attempt container escape, verify seccomp effectiveness
   - Credential handling: Design git proxy, validate no SSH keys in container
   - QEMU functionality: Build VM image, test launch, measure performance vs Docker

3. **Threat modeling session**:
   - Document attack vectors: container escape, credential theft, resource exhaustion
   - Prioritize mitigations: seccomp hardening, proxy design, resource limits
   - Define security test cases

4. **Proceed to Elaboration**:
   - Natural language: "Start Elaboration" or "Transition to Elaboration phase"
   - Explicit command: `/flow-inception-to-elaboration .`
   - Focus: Architecture refinement (proxy design), risk retirement (security testing), iteration planning

**Note**: You do NOT need to run `/intake-start` - these intake documents are complete and ready for immediate use in SDLC workflows.

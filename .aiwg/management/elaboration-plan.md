# Elaboration Phase Plan

**Project:** Agentic Sandbox
**Phase:** Elaboration (LCA Gate)
**Start:** 2026-01-24
**Target LCA:** 2026-02-07 (2 weeks)

## Objectives

1. **Validate architecture** through technical spikes
2. **Retire key risks** identified in Inception
3. **Produce executable architecture** (working skeleton)
4. **Refine iteration plan** for Construction phase

## Key Decisions from Inception

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Runtime | Docker + QEMU in parallel | Different isolation levels needed |
| Auth model | Gateway proxy (ADR-005) | "It just works" - no credentials in sandbox |
| Network | Filtered internet + MCP | Default deny, allowlist hosts |
| Deployment | Workstation → Server → Cluster | Progressive scaling |
| Invocation | CLI + API + Agent spawn | Multiple entry points |

## Technical Spikes (Week 1)

### Spike 1: Auth Gateway PoC
**Goal:** Validate auth injection pattern works with existing MCP servers
**Duration:** 2 days

Tasks:
- [ ] Deploy Envoy proxy with basic auth injection
- [ ] Configure routes for mcp-gitea, mcp-hound, mcp-memory
- [ ] Test from Docker container (agent perspective)
- [ ] Measure latency overhead
- [ ] Document configuration pattern

Success criteria:
- Agent in container can access all 4 MCP servers via gateway
- Auth tokens never visible inside container
- Latency < 5ms added per request

### Spike 2: Docker Runtime Hardening
**Goal:** Validate seccomp + capabilities + resource limits
**Duration:** 2 days

Tasks:
- [ ] Test PID limit enforcement (fork bomb defense)
- [ ] Validate seccomp profile blocks dangerous syscalls
- [ ] Test memory limit OOM behavior
- [ ] Test CPU throttling behavior
- [ ] Test network isolation (no direct egress)

Success criteria:
- Fork bomb contained at 1024 PIDs
- Memory OOM kills container cleanly
- No syscall escape possible
- Only gateway reachable from container

### Spike 3: QEMU/Firecracker Evaluation
**Goal:** Determine if Firecracker viable on workstation, or stick with QEMU
**Duration:** 2 days

Tasks:
- [ ] Check KVM availability on workstation
- [ ] Test Firecracker boot time and resource overhead
- [ ] Compare with QEMU boot time and overhead
- [ ] Test vsock communication for exec
- [ ] Evaluate image management (rootfs, kernel)

Success criteria:
- Clear recommendation: Firecracker vs QEMU for workstation
- Boot time < 2 seconds
- Memory overhead < 100MB per sandbox

### Spike 4: Runtime Abstraction Interface
**Goal:** Validate unified API works for both Docker and VM
**Duration:** 2 days

Tasks:
- [ ] Implement minimal RuntimeAdapter interface in Go
- [ ] Create Docker adapter (create, start, stop, exec)
- [ ] Create QEMU adapter (same operations)
- [ ] Test switching between runtimes
- [ ] Validate exec semantics match

Success criteria:
- Same API call creates sandbox on either runtime
- Exec works identically (stdin/stdout/stderr)
- Clean shutdown on both

## Architecture Refinement (Week 2)

### Deliverable 1: Executable Architecture
**Goal:** Working skeleton with all components integrated

Components:
- [ ] sandbox-manager (Go) - REST API server
- [ ] Docker adapter - container lifecycle
- [ ] QEMU adapter - VM lifecycle
- [ ] Auth gateway - Envoy or custom
- [ ] CLI tool - sandbox-cli

Integration test:
```bash
# Create sandbox (picks runtime automatically)
sandbox-cli create --name test-agent --image agent-base

# Exec command (via gateway to MCP)
sandbox-cli exec test-agent -- curl http://gateway/mcp-gitea/mcp

# Check status
sandbox-cli status test-agent

# Cleanup
sandbox-cli delete test-agent
```

### Deliverable 2: Network Architecture
**Goal:** Complete network isolation with gateway egress

Topology:
```
Host Network (172.16.0.0/16)
├── Gateway (172.16.0.1)
│   ├── Routes to MCP servers (internal)
│   └── Routes to allowed domains (external)
│
Sandbox Network (172.20.0.0/16)
├── Sandbox 1 (172.20.0.2) - Docker
├── Sandbox 2 (172.20.0.3) - Docker
└── Sandbox 3 (172.20.1.1) - QEMU (bridged)

Firewall Rules:
- Sandbox → Gateway: ALLOW (port 8080)
- Sandbox → anything else: DENY
- Gateway → MCP servers: ALLOW
- Gateway → allowlisted domains: ALLOW
```

### Deliverable 3: Construction Iteration Plan
**Goal:** Detailed plan for 8-week Construction phase

Proposed iterations:
1. **Iteration 1 (W1-2):** Docker runtime complete, CLI, basic API
2. **Iteration 2 (W3-4):** QEMU/Firecracker runtime, gateway hardening
3. **Iteration 3 (W5-6):** Python SDK, integration bridges
4. **Iteration 4 (W7-8):** Production hardening, documentation, security audit

## Risk Retirement

| Risk (from risk-list.md) | Spike | Mitigation Status |
|--------------------------|-------|-------------------|
| R-002: Credential leakage | Spike 1 | Gateway proves credentials never in sandbox |
| R-003: Resource exhaustion | Spike 2 | cgroups limits enforced |
| R-005: Network escape | Spike 2 | Isolated network validated |
| R-004: Container escape | Spike 2 | Seccomp + capabilities tested |

## LCA Gate Criteria

Lifecycle Completion Assessment (LCA) - end of Elaboration:

- [ ] All 4 spikes completed successfully
- [ ] Executable architecture running on workstation
- [ ] Auth gateway integrated with existing MCP servers
- [ ] Docker runtime hardened and tested
- [ ] QEMU/Firecracker decision made with rationale
- [ ] Construction iteration plan approved
- [ ] Major risks retired or mitigated
- [ ] Team confident to proceed to Construction

## Resources

**Existing MCP Servers:**
- mcp-gitea.integrolabs.net (Git operations)
- mcp-hound.integrolabs.net (Code search)
- memory.integrolabs.net (Memory/state)
- mcp-datagerry.integrolabs.net (IT assets/CMDB)

**Research Completed:**
- docs/research/platform-comparison.md
- docs/research/quick-reference-matrix.md
- docs/architecture/recommended-design.md

**ADRs:**
- ADR-001: Hybrid Runtime Approach
- ADR-002: Credential Proxy (superseded by ADR-005)
- ADR-003: Seccomp Design
- ADR-004: Network Isolation
- ADR-005: Auth Gateway (new)

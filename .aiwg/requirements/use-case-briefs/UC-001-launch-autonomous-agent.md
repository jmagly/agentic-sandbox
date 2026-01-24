# UC-001: Launch Autonomous Coding Agent

## Use Case Overview

**ID**: UC-001
**Priority**: Critical
**Status**: Active
**Last Updated**: 2026-01-05

## Summary

Developer launches a long-running autonomous coding task in an isolated sandbox environment, enabling the agent to work for hours or days without compromising host security or requiring manual intervention.

## Actors

**Primary**: Developer
**Secondary**: Claude Code Agent (automated)
**Supporting**: Sandbox Manager, Docker Runtime

## Stakeholders and Interests

- **Developer**: Wants autonomous task completion without manual monitoring, secure isolation, reliable output persistence
- **Security Team**: Requires host isolation, credential protection, audit logging
- **Operations**: Needs resource limits to prevent host exhaustion

## Preconditions

- Docker Engine 24+ installed and running on host
- Agent image (agent-claude) built and available
- Anthropic API key configured in environment or secrets
- Sufficient host resources available (4 CPU, 8GB RAM minimum)
- Workspace directory exists and has appropriate permissions

## Postconditions

**Success**:
- Container launched and running in isolated environment
- Agent executing autonomous task with full development tooling
- Workspace persisted to host volume for output retrieval
- Security hardening active (seccomp, capabilities, network isolation)
- Audit logs capturing agent activity

**Failure**:
- Container fails to launch with clear error message
- Resources cleaned up (no orphaned containers)
- Error logged for troubleshooting

## Main Success Scenario

1. Developer runs: `./scripts/sandbox-launch.sh --runtime docker --image agent-claude --task "Refactor authentication module to use OAuth 2.0"`
2. Sandbox launcher validates parameters and checks Docker availability
3. System creates isolated Docker container with security hardening:
   - Seccomp syscall filtering applied
   - All Linux capabilities dropped, minimal re-added
   - Isolated network bridge created (no external access by default)
   - Resource limits enforced (4 CPU, 8GB memory)
4. Agent image pulled (if not cached) and container started
5. Claude Code CLI initializes inside container with API key from secrets
6. Agent begins autonomous execution of refactoring task
7. Agent works for hours/days, persisting outputs to /workspace volume
8. Developer retrieves completed work from workspace directory
9. Container continues running until task completion or manual stop

**Expected Duration**: Container launch <30s, task execution hours to days

## Alternative Flows

**2a. Docker not available**:
- System displays error: "Docker Engine not running or not installed"
- Exits with status code 1

**3a. Insufficient host resources**:
- Docker fails to allocate resources
- System displays error with current resource usage
- Suggests reducing CPU/memory limits or stopping other containers

**4a. Image not available locally**:
- System attempts to pull from container registry
- If pull fails (network issue, auth failure), displays error
- Developer can manually build image and retry

**5a. API key missing or invalid**:
- Container starts but Claude Code CLI fails initialization
- Error logged to container logs
- Container exits with error status

**6a. Agent encounters unrecoverable error**:
- Agent logs error details to workspace
- Container remains running for debugging
- Developer can inspect logs via `docker logs`

**8a. Developer needs to stop task early**:
- Developer runs `docker stop <container-id>`
- Agent receives SIGTERM, graceful shutdown
- Workspace preserved with partial outputs

## Exception Flows

**E1. Container escape attempt detected**:
- Seccomp profile blocks dangerous syscall
- Audit log records attempted syscall
- Container continues running (no crash)
- Alert triggered for security review

**E2. Resource exhaustion (fork bomb, memory leak)**:
- cgroups enforce limits, prevent host impact
- Container may become unresponsive
- Developer kills container manually
- Workspace preserved for analysis

**E3. Network isolation bypass attempt**:
- iptables rules block unauthorized egress
- Agent receives network error
- Audit log records connection attempt

## Business Rules

**BR-001**: Maximum container runtime defaults to 7 days (604800 seconds)
**BR-002**: Agent must run as non-root user (UID 1000, username: agent)
**BR-003**: API keys must be injected via Docker secrets, never environment variables
**BR-004**: Workspace must be mounted read-write for output persistence
**BR-005**: Seccomp profile must block dangerous syscalls (reboot, module loading, kexec)

## Special Requirements

### Performance
- Container launch latency: <30 seconds from command to agent ready
- Workspace I/O: Near-native disk speeds (NVMe SSD backed)
- No noticeable host performance degradation with 5 concurrent sandboxes

### Security
- Seccomp syscall filtering enabled (200+ allowed syscalls, dangerous blocked)
- Linux capabilities: ALL dropped, minimal re-added (NET_BIND_SERVICE, CHOWN, SETUID, SETGID)
- Network isolation: Internal bridge only, no external access without explicit configuration
- Read-only root filesystem: Optional (disabled by default for flexibility)
- Audit logging: All container lifecycle events and syscall violations

### Usability
- Single command launch with sensible defaults
- Clear error messages for common failures
- Detached mode for background execution
- Easy access to logs via standard Docker commands

## Technology and Data Variations

**Runtime Variations**:
- Docker (primary): Fast launch, shared kernel
- QEMU (alternative): Full VM isolation, slower launch

**Image Variations**:
- agent-claude: Full development environment with Claude Code CLI
- agent-base: Minimal tooling for custom agent implementations

**Storage Variations**:
- Local volume mount: Direct host filesystem access
- Named Docker volume: Managed by Docker, portable
- tmpfs: In-memory workspace for sensitive data (no persistence)

## Open Issues

**OI-001**: GPU passthrough for Docker containers not yet supported (QEMU only)
**OI-002**: Checkpoint/resume for long-running tasks not implemented
**OI-003**: Multi-container agent coordination (message queues) not yet available
**OI-004**: Web UI for sandbox management deferred to future phase

## Frequency of Occurrence

- **Expected**: 10-20 sandbox launches per week per developer
- **Peak**: 5-10 concurrent sandboxes per developer for large refactoring projects
- **Team-wide**: 50-100 launches per week for 5 developers

## Assumptions

**A-001**: Developer has local Docker access and basic CLI familiarity
**A-002**: Host has sufficient resources for at least 3 concurrent sandboxes
**A-003**: Anthropic API has no rate limits affecting multi-hour agent execution
**A-004**: Agent code is semi-trusted (developed internally or vetted third-party)
**A-005**: Workspace data does not require encryption at rest (host security sufficient)

## Acceptance Criteria

- [ ] Container launches successfully in <30 seconds
- [ ] Agent executes task autonomously without manual intervention
- [ ] Workspace outputs available on host filesystem after task completion
- [ ] No credentials (API keys, SSH keys) visible in container filesystem or logs
- [ ] Seccomp profile blocks attempted reboot syscall (security test)
- [ ] cgroups enforce CPU/memory limits (prevent host exhaustion)
- [ ] Audit log captures container start, stop, and task completion events
- [ ] Agent can run for 24+ hours without container restart
- [ ] Multiple sandboxes (3+) run concurrently without interference

## Notes

- This is the most critical use case for project success
- Security validation focuses on container escape prevention and credential protection
- Future enhancement: Automatic workspace backup to S3 after task completion
- Consider adding: Progress notifications via webhook when task completes

## Related Use Cases

- **UC-002**: Git Repository Operations via Proxy (agents need git access)
- **UC-004**: Resource-Limited Sandbox Execution (enforces limits tested here)
- **UC-005**: Persistent Workspace Across Sessions (workspace survival tested here)

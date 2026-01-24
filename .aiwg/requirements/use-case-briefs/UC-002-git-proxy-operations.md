# UC-002: Git Repository Operations via Proxy

## Use Case Overview

**ID**: UC-002
**Priority**: High
**Status**: Planned (Integration bridge not yet implemented)
**Last Updated**: 2026-01-05

## Summary

Agent inside sandbox performs git operations (clone, commit, push) on remote repositories without direct access to SSH keys or HTTPS tokens. Credentials remain on host and are injected via authenticated proxy.

## Actors

**Primary**: Agent (inside sandbox)
**Secondary**: Git Proxy Service (on host)
**Supporting**: GitHub/GitLab/Gitea (remote git hosting)

## Stakeholders and Interests

- **Security Team**: Requires zero credential exposure inside sandbox
- **Agent**: Needs transparent git operations without credential management
- **Developer**: Wants audit trail of all git operations
- **Operations**: Needs reliable proxy service with minimal failure modes

## Preconditions

- Git proxy service running on host (listening on internal bridge network)
- Host git credentials configured (SSH keys or HTTPS tokens)
- Agent has git client installed
- Agent git config points to proxy endpoint (http://git-proxy:8080)
- Network connectivity between sandbox and proxy established

## Postconditions

**Success**:
- Repository cloned to sandbox workspace
- Agent can commit, push, pull, fetch without credential prompts
- No SSH keys or tokens stored in container filesystem or environment
- Audit log captures all git operations (repo, operation, timestamp, agent ID)

**Failure**:
- Clear error message indicating proxy unavailable or authentication failed
- No partial repository state (clone is atomic)
- Agent can retry operation

## Main Success Scenario

1. Agent executes: `git clone http://git-proxy:8080/org/repo.git`
2. Git client sends HTTP request to proxy on internal network
3. Git proxy receives request, parses target repository URL
4. Proxy authenticates to GitHub using host SSH key (never exposed to container)
5. Proxy streams repository data to agent over HTTP
6. Repository cloned to sandbox workspace at /workspace/repo
7. Agent modifies code, commits changes locally
8. Agent executes: `git push origin main`
9. Git client sends push request to proxy
10. Proxy authenticates to GitHub, pushes changes
11. GitHub confirms push success
12. Proxy returns success to agent
13. Audit log records: [timestamp] Agent <agent-id> pushed to org/repo branch main

**Expected Duration**: Clone <10s for typical repo, push <5s

## Alternative Flows

**3a. Proxy parses unsupported protocol (git://, ssh://)**:
- Proxy returns error: "Only HTTP(S) proxying supported"
- Agent receives error, can retry with HTTP URL

**4a. Host credentials expired or invalid**:
- Proxy fails to authenticate to GitHub
- Proxy returns error: "Authentication failed: check host credentials"
- Developer updates host credentials, agent retries

**6a. Repository too large for workspace disk quota**:
- Clone fails with disk quota exceeded error
- Proxy aborts transfer
- Agent receives error, no partial clone left behind

**8a. Push conflicts with remote changes**:
- GitHub rejects push (non-fast-forward)
- Proxy forwards error to agent
- Agent can pull, rebase, retry push

**9a. Git proxy service down**:
- Agent receives connection refused error
- Agent can retry with exponential backoff
- Alert triggered for operations team to restart proxy

## Exception Flows

**E1. Agent attempts to bypass proxy (direct GitHub connection)**:
- Network isolation blocks outbound HTTPS to github.com
- Agent receives connection timeout or network unreachable
- Audit log records unauthorized connection attempt

**E2. Agent attempts to read proxy configuration for credentials**:
- Proxy runs on host, outside container namespace
- Agent has no access to proxy process memory or config files
- Attempt fails, no credential exposure

**E3. Proxy process crashes mid-operation**:
- Agent receives broken pipe or connection reset
- Partial clone/push state cleaned up
- Systemd auto-restarts proxy service
- Agent retries operation after backoff

**E4. Rate limit exceeded on GitHub API**:
- GitHub returns 429 Too Many Requests
- Proxy forwards error to agent with retry-after header
- Agent waits specified duration, retries automatically

## Business Rules

**BR-001**: All git operations must transit proxy (no direct remote access)
**BR-002**: Proxy must log operation metadata without logging code diffs
**BR-003**: SSH keys and tokens never exposed to container environment or filesystem
**BR-004**: Proxy must support both HTTPS and SSH upstream (git@ URLs)
**BR-005**: Rate limiting enforced: max 100 git operations per agent per hour

## Special Requirements

### Performance
- Clone throughput: Match host network speeds (100+ Mbps)
- Proxy overhead: <100ms latency added vs direct git operation
- Concurrent operations: Support 10+ agents performing simultaneous git operations

### Security
- Credentials isolation: Zero credential artifacts in container (verified via filesystem scan)
- TLS termination: Proxy uses HTTPS to GitHub even if agent uses HTTP internally
- Audit completeness: 100% of git operations logged with repo, operation, timestamp, agent ID
- Credential rotation: Host credentials can be updated without container restart

### Reliability
- Proxy availability: 99.9% uptime (systemd auto-restart on crash)
- Operation atomicity: Clone/push either fully succeeds or fully rolls back
- Retry logic: Agent automatically retries on transient failures (network, rate limit)

## Technology and Data Variations

**Protocol Variations**:
- HTTPS proxy: Agent uses HTTP, proxy upgrades to HTTPS for GitHub
- SSH proxy: Agent uses git@ URLs, proxy handles SSH key authentication

**Authentication Methods**:
- GitHub SSH key: Proxy uses host SSH key, supports deploy keys
- GitHub Personal Access Token: Proxy uses PAT in HTTPS Authorization header
- GitLab/Gitea: Same proxy model, different API endpoints

**Repository Types**:
- Public repositories: No authentication required, proxy still routes for audit logging
- Private repositories: Proxy authenticates, agent never sees credentials
- Monorepos: Large repositories may require streaming clone with progress reporting

## Open Issues

**OI-001**: Git LFS (Large File Storage) support not yet implemented
**OI-002**: Shallow clone optimization (--depth) not tested with proxy
**OI-003**: Submodule handling may require recursive proxy configuration
**OI-004**: Git credential helper integration unclear (may interfere with proxy model)
**OI-005**: Performance impact of proxy on large binary file operations unknown

## Frequency of Occurrence

- **Expected**: 5-10 git operations per agent per day (clone once, push 3-5 times)
- **Peak**: 50+ operations during large refactoring (frequent commits, pushes)
- **Team-wide**: 100-200 git operations per day for 10 concurrent agents

## Assumptions

**A-001**: Git proxy service has access to host credentials (SSH keys, PATs)
**A-002**: Internal network between container and proxy is trusted (no TLS required)
**A-003**: GitHub/GitLab APIs remain stable (no breaking changes to git protocol)
**A-004**: Agent respects git proxy configuration (no hardcoded github.com URLs)
**A-005**: Host has sufficient bandwidth for concurrent large repository clones

## Acceptance Criteria

- [ ] Agent can clone private repository without SSH key in container
- [ ] Agent can push commits to remote via proxy
- [ ] Agent can pull updates, fetch branches, create tags via proxy
- [ ] No SSH keys or tokens found in container filesystem (security scan)
- [ ] No credentials in container environment variables (env inspection)
- [ ] Audit log captures 100% of git operations with metadata
- [ ] Proxy survives crash and auto-restarts via systemd
- [ ] Agent retries failed operations with exponential backoff
- [ ] Concurrent operations from 5 agents succeed without interference
- [ ] Host credential rotation does not require container restart

## Notes

- This is the highest-priority integration bridge (most common agent operation)
- Prototype should focus on GitHub HTTPS first (simplest implementation)
- SSH proxy requires SSH key forwarding without exposing private key
- Consider using existing tools (git-remote-http helper) vs custom proxy
- Audit log must not capture code diffs (only metadata: repo, branch, commit hash)

## Related Use Cases

- **UC-001**: Launch Autonomous Coding Agent (agents need git access for code tasks)
- **UC-003**: Secure VM Sandbox for Untrusted Agent (same proxy model applies to VMs)
- **UC-005**: Persistent Workspace Across Sessions (cloned repos persist in workspace)

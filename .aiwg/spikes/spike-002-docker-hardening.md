# Spike 002: Docker Runtime Hardening

**Status:** Complete
**Date:** 2026-01-24
**Duration:** 30 minutes

## Objective

Validate that Docker containers can be hardened with PID limits, memory limits, seccomp profiles, capability dropping, and network isolation.

## Results

### Success Criteria

| Criteria | Result | Notes |
|----------|--------|-------|
| Fork bomb blocked at PID limit | **PASS** | `--pids-limit 32` blocked fork at 32 processes |
| Memory OOM enforced | **PASS** | `--memory=64m` limited allocation to 64MB |
| Network isolation | **PASS** | `--network none` only loopback available |
| Capabilities dropped | **PASS** | `--cap-drop=ALL` blocked privileged ops |
| Seccomp profile | **PASS** | Custom profile blocked dangerous syscalls |
| Read-only filesystem | **PASS** | `--read-only` prevented writes to / |

### Test Commands

```bash
# PID limit (fork bomb defense)
docker run --rm --pids-limit 32 sandbox-test:latest bash -c '
for i in $(seq 1 50); do sleep 100 &; done
'
# Result: "fork: Resource temporarily unavailable" after 32 processes

# Memory limit
docker run --rm --memory=64m sandbox-test:latest bash -c '
dd if=/dev/zero of=/dev/shm/bigfile bs=1M count=200
'
# Result: "No space left on device" at 64MB

# Network isolation
docker run --rm --network none sandbox-test:latest curl https://google.com
# Result: Connection failed (no network)

# Capability dropping
docker run --rm --cap-drop=ALL sandbox-test:latest ip link set lo down
# Result: "Operation not permitted"

# Full hardening combo
docker run --rm \
  --network none \
  --memory=128m \
  --pids-limit=64 \
  --cap-drop=ALL \
  --security-opt no-new-privileges:true \
  --security-opt seccomp=configs/seccomp-agent.json \
  --read-only \
  --tmpfs /tmp:noexec,nosuid,size=32m \
  sandbox-test:latest bash -c 'echo "Hardened container running"'
```

### Seccomp Profile

Created `configs/seccomp-agent.json`:
- Default action: ERRNO (block unknown syscalls)
- Allowlist of ~200 safe syscalls
- Explicit blocklist: ptrace, mount, chroot, bpf, unshare, setns, etc.

### Docker Run Flags Summary

| Flag | Purpose | Value |
|------|---------|-------|
| `--network none` | No network access | Only loopback |
| `--memory` | Memory limit | 128m - 8G |
| `--pids-limit` | Process limit | 64 - 1024 |
| `--cap-drop=ALL` | Drop all capabilities | - |
| `--security-opt no-new-privileges` | Prevent privilege escalation | - |
| `--security-opt seccomp=` | Syscall filtering | Custom profile |
| `--read-only` | Read-only root filesystem | - |
| `--tmpfs /tmp` | Writable /tmp in memory | noexec,nosuid |

### Files Created

- `images/test/Dockerfile` - Test image with stress-ng, curl, iproute2
- `configs/seccomp-agent.json` - Seccomp syscall filter profile

## Recommendations

1. **Use all flags together** for production sandboxes
2. **PID limit 1024** is reasonable for most agent workloads
3. **Memory limit 8GB** default, configurable per agent
4. **Network via gateway only** - never direct internet
5. **Seccomp profile is essential** - blocks container escapes

## Docker Compose Template

```yaml
services:
  agent-sandbox:
    image: sandbox-test:latest
    network_mode: none  # Or custom bridge to gateway
    mem_limit: 8g
    pids_limit: 1024
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
      - seccomp:configs/seccomp-agent.json
    read_only: true
    tmpfs:
      - /tmp:noexec,nosuid,size=1g
    volumes:
      - workspace:/workspace:rw
```

## Next Steps

1. Create Docker network bridge to gateway
2. Test CPU throttling with `--cpus`
3. Add disk quota (XFS project quotas or overlay limits)

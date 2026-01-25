# Spike 004: Runtime Abstraction Interface

**Status:** Complete
**Date:** 2026-01-24
**Duration:** 30 minutes

## Objective

Create a unified launcher script that abstracts Docker and QEMU runtimes behind a common interface.

## Implementation

Enhanced `scripts/sandbox-launch.sh` with:

### Features

1. **Unified CLI for both runtimes**
   - `--runtime docker` - Container-based sandbox
   - `--runtime qemu` - VM-based sandbox

2. **Security hardening (Docker)**
   - PID limits (`--pids-limit`)
   - Memory limits (`--memory`)
   - Capability dropping (`--cap-drop ALL`)
   - Seccomp profile (auto-detected)
   - Read-only root filesystem
   - No-new-privileges

3. **Network modes**
   - `isolated` - No network (default)
   - `gateway` - Access via auth-injecting gateway
   - `host` - Full access (not recommended)

4. **QEMU integration**
   - Loads VM config from `runtimes/qemu/*.xml`
   - Adjusts memory/CPU from command line
   - Supports console attach or detached mode

### Usage Examples

```bash
# Isolated Docker sandbox (most secure)
./scripts/sandbox-launch.sh --runtime docker --image sandbox-test

# Docker with gateway access to MCP servers
./scripts/sandbox-launch.sh --runtime docker --image sandbox-test \
    --network gateway --gateway http://gateway:8080 \
    --mount ./workspace:/workspace

# QEMU VM with GPU passthrough
./scripts/sandbox-launch.sh --runtime qemu --image ubuntu-agent \
    --memory 16G --cpus 8 --gpu passthrough
```

### Runtime Selection Logic

```
User specifies --runtime
       ↓
   docker → Docker adapter with hardening
   qemu   → QEMU/libvirt adapter
```

Both adapters produce sandboxes with:
- Isolated network (or gateway-only access)
- Resource limits enforced
- Non-root execution
- Minimal attack surface

## Files Updated

- `scripts/sandbox-launch.sh` - Enhanced with security hardening, network modes, gateway support

## Abstraction Interface

The script implements a simple runtime abstraction:

| Function | Docker | QEMU |
|----------|--------|------|
| Create | `docker run` | `virsh define` |
| Start | (implicit) | `virsh start` |
| Stop | `docker stop` | `virsh destroy` |
| Exec | `docker exec` | `virsh console` |
| Resource limits | cgroups via Docker | libvirt XML |
| Network isolation | `--network none` | Isolated bridge |

## Next Steps

For a full REST API implementation (as designed in research):

1. **Go-based sandbox manager** - REST API server
2. **Docker adapter** - Full lifecycle management
3. **QEMU adapter** - libvirt integration
4. **Python SDK** - Client library

The bash script provides the core functionality for the PoC phase.

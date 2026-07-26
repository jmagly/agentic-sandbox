# Container Runtime Security Boundary

Managed Docker sandboxes are a process-isolation tier, not a virtual-machine
boundary. They share the Docker host kernel, so kernel version and broad kernel
behavior may be fingerprinted from inside a container. Workloads that require a
separate kernel or stronger tenant isolation must use the QEMU/KVM runtime.

The managed-container baseline is enforced by the management service:

- dedicated numeric identity `10001:10001`;
- all Linux capabilities dropped;
- `no-new-privileges` enabled;
- one user-defined bridge per sandbox unless an operator explicitly supplies a
  network;
- bootstrap bearer tokens streamed over stdin into a `noexec,nosuid,nodev`
  tmpfs file, never placed in Docker arguments or container configuration;
- enrollment keys and certificates written to that same runtime-only tmpfs.

The minimal base image intentionally omits `grpcurl`, `curl`, `wget`, and
Python. Development/loadout images may include such tools.
Tool absence is defense in depth and is not treated as an isolation boundary.

Run the sanitized verifier with:

```bash
./scripts/verify-container-security.sh
AGENTIC_SECURITY_IMAGE=agentic/base:local ./scripts/verify-container-security.sh
```

Set `AGENTIC_REQUIRE_DEDICATED_AGENTSHARE=1` in production validation to require
the agentshare root to reside on a device distinct from `/`. The verifier emits
only pass/fail control evidence and never environment contents, tokens,
transcripts, or host inventory.

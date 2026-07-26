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
- enrollment keys and certificates written with private modes under the
  container user's home so they survive a managed stop/start but remain scoped
  to that container's writable layer and are removed with container destroy.

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

Run logs and transcripts under agentshare are sensitive. Their host-visible
directories and files are created with modes `0700` and `0600`. The default
retention policy is seven days after the run directory's last modification;
the active `current` run is never eligible. Preview or apply the policy with:

```bash
AGENTSHARE_ROOT=/srv/agentshare ./scripts/prune-agentshare-runs.sh
sudo AGENTSHARE_ROOT=/srv/agentshare ./scripts/prune-agentshare-runs.sh --apply
```

Operators may set `AGENTIC_RUN_RETENTION_DAYS` or pass `--retention-days`.
Applying the policy deletes the complete expired run directory, including
`stdout.log`, `stderr.log`, `commands.log`, metrics, outputs, and traces.
Deletion does not promise physical-media overwriting on copy-on-write, flash,
or thin-provisioned storage; stronger sanitization requires encrypted storage
and destruction of the retired encryption key.

# Container Runtime Security Boundary

Managed Docker sandboxes are a process-isolation tier, not a virtual-machine
boundary. They share the Docker host kernel, so kernel version and broad kernel
behavior may be fingerprinted from inside a container. Workloads that require a
separate kernel or stronger tenant isolation must use the QEMU/KVM runtime.

The managed-container baseline is enforced by the management service:

- dedicated numeric identity `10001:10001`;
- all Linux capabilities dropped;
- `no-new-privileges` enabled;
- one internal user-defined bridge per sandbox, with no Docker-provided
  external route, unless an operator explicitly supplies a network;
- bootstrap bearer tokens streamed over stdin into a `noexec,nosuid,nodev`
  tmpfs file, never placed in Docker arguments or container configuration;
- enrollment keys and certificates written with private modes under the
  container user's home so they survive a managed stop/start but remain scoped
  to that container's writable layer and are removed with container destroy.

The last item is persistence and cross-container separation, not
workload/credential-owner separation: the agent process and workload currently
share UID `10001`. A workload that can execute arbitrary code as that UID may
read credential material made available to the agent or provider CLI. This is
the SBX-002 limitation tracked in #617. Until transport and provider operations
are held by a distinct broker identity, live-credential container use remains
a T0 developer/bench posture and must not be described as a T1 security
boundary.

The minimal base image intentionally omits `grpcurl`, `curl`, `wget`, and
Python. Development/loadout images may include such tools.
Tool absence is defense in depth and is not treated as an isolation boundary.

## Network trust boundary

The managed default is a Docker `--internal` network. It permits traffic within
that sandbox network but does not install Docker's normal external-egress
route. Each managed network carries the
`agentic-egress-policy=default-deny` label so host-side inventory can distinguish
the enforced posture without inspecting workload traffic.

Supplying `network` in the container provision request is an explicit
operator-controlled compatibility escape hatch. Agentic Sandbox does not
rewrite or validate an operator-supplied Docker network, so that mode is a T0
developer/bench posture and must not be presented as an end-user security
boundary. T1 and higher claims require the managed internal network or an
independently enforced, audited egress gateway. The HTTP credential proxy can
mediate allowlisted upstream calls, but it is not by itself proof that direct
egress is blocked.

The default internal network closes arbitrary public egress; it does not yet
provide destination and byte-count audit for selectively allowed traffic.
Deployments needing allowlisted public endpoints must attach an independently
managed filtering gateway and retain its decision metadata without payload or
credential content.

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

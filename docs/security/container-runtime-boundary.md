# Container Runtime Security Boundary

Managed Docker sandboxes are a process-isolation tier, not a virtual-machine
boundary. They share the Docker host kernel, so kernel version and broad kernel
behavior may be fingerprinted from inside a container. Workloads that require a
separate kernel or stronger tenant isolation must use the QEMU/KVM runtime.

The default managed-container baseline is enforced by the management service:

- a unique per-instance control identity in the numeric UID range
  `200000..799999`, distinct from workload UID/GID `10001:10001`;
- a host-mounted gRPC Unix-domain socket. Management authenticates the
  connection with `SO_PEERCRED` and a dynamic UID-to-instance map, so no
  bootstrap bearer, client certificate, or private key enters the container;
- the control process retains only `SETUID` and `SETGID`, solely to create
  workload children. Each child enters `10001:10001` and clears ambient,
  effective, permitted, and inheritable capabilities before exec;
- `no-new-privileges` enabled;
- on Linux, one internal user-defined bridge per sandbox, with no
  Docker-provided external route, unless an operator explicitly supplies a
  network;
- a private `noexec,nosuid,nodev` control tmpfs for the setup sentinel;
- unknown, duplicate, or workload UIDs rejected by the UDS identity resolver;
- admin-v2 Docker startup profiles that contain raw credential references
  rejected before provisioning. Use the credential proxy or a VM runtime.

This closes the default-path transport-key exposure from SBX-002 (#617): the
workload UID neither owns the control process nor maps to a UDS agent identity,
and there is no mTLS private key to read. It does not make arbitrary provider
credentials safe inside the workload. A provider token deliberately supplied
through the v1 `env` field, an operator-selected transport/network, a manual
provider login, or a direct `docker exec` remains an explicit T0
developer/bench posture. Raw-token-free T1 claims require a supported
credential-proxy flow; providers that require local raw material should use a
VM runtime.

The minimal base image intentionally omits `grpcurl`, `curl`, `wget`, and
Python. Development/loadout images may include such tools.
Tool absence is defense in depth and is not treated as an isolation boundary.

## Network trust boundary

The managed Linux default is a Docker `--internal` network. It permits traffic
within that sandbox network but does not install Docker's normal
external-egress route. Each Linux managed network carries the
`agentic-egress-policy=default-deny` label so host-side inventory can distinguish
the enforced posture without inspecting workload traffic.

Docker Desktop cannot route `host.docker.internal` from an `--internal`
network, including when an explicit `host-gateway` mapping is present. The
macOS preview runtime therefore uses a normal managed bridge so its agent can
reach the host management callback and labels that network
`agentic-egress-policy=unrestricted-platform-compatibility`. This is explicitly
a T0 developer/bench posture: macOS Docker workloads can reach public
destinations and must not receive live credentials or be described as a T1
egress boundary.

Supplying `network` in the container provision request is an explicit
operator-controlled compatibility escape hatch. Agentic Sandbox does not
rewrite or validate an operator-supplied Docker network, so that mode is a T0
developer/bench posture and must not be presented as an end-user security
boundary. T1 and higher claims require the Linux managed internal network or an
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

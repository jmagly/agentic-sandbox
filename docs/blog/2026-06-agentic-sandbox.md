# June 2026: Secure runtimes, live Observe/Drive, and a cleaner release surface

**Published:** 2026-06-30  
**Project:** Agentic Sandbox  
**Window:** June 2026  
**Latest release covered:** [v2026.6.36](../releases/v2026.6.36.md)

Agentic Sandbox gives AI agents a controlled place to do real work. They can
run code, use a terminal, keep state, and operate inside either a fast container
or a stronger virtual machine boundary without getting unrestricted access to
the rest of the host.

June was a security and reliability month. The core story is simple: the
connection between the control plane and the agent runtime is now private and
verified by default, VM-backed agents are much more dependable, operators can
use short-lived SSH access through a gateway, and live terminal Observe/Drive
now has a clean release with a complete public image matrix.

## TL;DR

- Secure transport is now the default path for agent connections.
- VM-backed agents can enroll over private host-to-guest paths instead of
  depending on broad network reachability.
- SSH access now goes through a gateway that issues short-lived, scoped access.
- Live terminal observation and control were hardened across the pty-ws bridge.
- Linux packages, installer assets, SBOMs, and seven public GHCR images are
  published from the current release flow.
- [v2026.6.36](../releases/v2026.6.36.md) supersedes
  [v2026.6.35](../releases/v2026.6.35.md) for new installs because it completed
  the public container publication surface.

## By the numbers

| Public surface | Current state |
| --- | --- |
| Runtime choices | Container, QEMU/KVM VM, and host runtime tier |
| Install paths | Linux installer, `.deb`, and `.rpm` packages |
| Public images | Seven GHCR images for management, agent client, agent, Claude, Codex, OpenCode, and automation-control |
| Verification | Checksums, installer dry-run validation, release SBOMs, and public image manifest checks |
| Current release | `v2026.6.36` |
| Docs | `https://docs.aiwg.io/agentic-sandbox/` |
| Source | `https://github.com/jmagly/agentic-sandbox` |

## Highlights

### Locked doors by default

The link between the management plane and an agent sandbox now uses verified,
private transport by default. Instead of relying on legacy shared secrets or
trust-on-first-use behavior, each side proves its identity through the transport
identity path.

That matters because agents can run real commands. The doorway into that
runtime should be locked by default, not left to operators to remember later.

### VM agents that check in reliably

The VM path received a lot of attention this month. Agent enrollment now works
over same-host private paths such as vsock, so a QEMU guest can come up and
check in without depending on a loopback-reachable network route back to the
management server.

The surrounding VM lifecycle work also improved cleanup: CID ownership, teardown
state, DHCP cleanup, and E2E VM reaping were hardened so old state is less
likely to leak into the next run.

### Short-lived SSH through a gateway

Operators can now reach sandboxes through routed SSH access backed by short-lived
leases. The gateway can issue scoped access, track it, and revoke it instead of
asking teams to keep long-lived keys around.

The practical result is a safer operational path: get shell access when you need
it, let it expire when you do not, and keep an audit trail around the access.

### Live Observe/Drive that stays live

The live terminal stack now has a stronger output and control path. The
`v2026.6.35` release fixed a pty-ws bridge issue where VM session output could
go quiet after the bridge handoff, even though the underlying session was still
running.

It also added heartbeat-based stale-controller cleanup. If a browser tab,
network path, or client process disappears without a clean detach, the executor
can reap that controller slot so the next authorized client can take control
instead of being stuck as an observer.

### Credential proxy groundwork

June also added the first HTTP credential-proxy backend. Managed sessions can
call approved upstream HTTP/API targets through an active credential lease while
the secret is injected only on the management-side outbound request. Policy
mismatches fail closed, and returned bodies are redacted.

This is the direction the project is taking for agent credentials: short-lived,
scoped, policy-bound access instead of broad secrets inside the workload.

### Release publication got stricter

The month ended with a useful release lesson. `v2026.6.35` carried the runtime
payload for live Observe/Drive reliability, but a transient GHCR `unknown blob`
error left the public image mirror incomplete.

`v2026.6.36` kept the same runtime payload and hardened the public registry
mirror step with retries for `docker pull` and `docker push`. The follow-up tag
completed the release surface: Linux packages, installer assets, SBOMs, GitHub
release mirroring, and all seven public GHCR images.

## What shipped

### Secure transport and identity

The secure connection work was the center of the month. Agent connections moved
away from weaker defaults and toward explicit identity, certificate-backed
transport, and fail-closed behavior. The system can use different underlying
paths depending on where the runtime lives: network transport, local sockets, or
same-host VM transport.

### VM and base-image reliability

The QEMU path became more automation-friendly. Base image builds got safer
overwrite behavior and fallback handling for hosts with restricted `/boot`
access. VM teardown and registry cleanup paths were tightened so repeated
provision/destroy cycles behave more predictably.

### Live terminal transport

Terminal output now has a more reliable bridge from the executor into the
canonical event stream. Operators can attach as observers or controllers, and
stale control sessions are cleaned up when clients stop responding to WebSocket
Ping frames.

### Packages and public images

The current public release surface is Linux-first: installer script, `.deb`,
`.rpm`, release tarballs, SBOMs, and GHCR images. Darwin/macOS artifacts are
deferred from the required publication gate for now, so Linux packages and
public images are the active release proof.

## Releases

June had a steady stream of small releases. Many were deliberate follow-ups that
fixed the release before them, keeping the public audit trail explicit.

- **2026.6.0** - moved the end-to-end test suite to Rust and added live-agent
  and restart-durability checks.
- **2026.6.1** - completed secure-connection groundwork and removed legacy
  insecure defaults.
- **2026.6.13** - moved Apple Silicon host checks onto an existing path.
- **2026.6.14** - hardened release SBOM and attachment behavior.
- **2026.6.15** - made signing and SBOM tooling build from a cleaner local
  location.
- **2026.6.16** - mirrored Linux package checksums to the public GitHub release.
- **2026.6.19** - fixed the host runtime path and superseded the previous cut.
- **2026.6.24** - repaired Docker and VM runtime publication paths.
- **2026.6.25** - allowed freshly enrolled agents to register over verified
  transport and exposed transport posture in fleet views.
- **2026.6.26** - moved Linux ARM64 builds off a flaky runner.
- **2026.6.27** - added a fast path for high-volume live terminal output.
- **2026.6.28** - delivered gateway-backed SSH access with tracked leases.
- **2026.6.29** - published the OWASP-aligned security profile.
- **2026.6.30** - let the admin API accept an SSH key when starting an instance.
- **2026.6.31** - fixed same-host VM agent check-in over private transport.
- **2026.6.32** - finished the release flow for the VM connection work.
- **2026.6.33** - hardened release runners and image provenance checks.
- **2026.6.34** - repaired QEMU/vsock edge cases and base-image build fallback
  behavior.
- **2026.6.35** - shipped the Observe/Drive reliability payload: pty-ws bridge
  output, stale-controller cleanup, vsock cleanup hardening, and HTTP credential
  proxy.
- **2026.6.36** - republished the same runtime payload with retry-hardened
  public image mirroring and a complete GHCR release matrix.

## Breaking changes and migrations

- Legacy shared-secret and trust-on-first-use transport defaults are no longer
  the expected path. Operators should use the secure transport identity flow.
- The old Python E2E harness was retired in favor of Rust-based E2E coverage.
- For new installs and upgrades, prefer `v2026.6.36` over `v2026.6.35`; the
  latter had a partial public container mirror even though its runtime payload
  was valid.

## Known threads

- Darwin/macOS release assets are intentionally outside the required release
  gate while the Linux package and public image surface remains the active
  publication target.
- VM startup and teardown paths continue to get hardening because they are the
  strongest isolation tier and the most host-sensitive runtime.
- Credential proxy support starts with HTTP/API calls and should continue
  expanding around scoped, auditable access patterns.

## What is next

Expect more hardening around VM lifecycle, live terminal recovery, credential
lease policy, and release verification. The main direction is unchanged: agents
get real execution environments, while operators keep transport identity,
credential scope, terminal visibility, and release provenance under control.

## Links

- [Getting started](../getting-started.md)
- [Release verification](../releases/verification.md)
- [v2026.6.36 release notes](../releases/v2026.6.36.md)
- [Security status](../security/security-status.md)
- [ASVS profile](../security/asvs-profile.md)


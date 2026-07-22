# ADR-032: Ship Apple Silicon Host and Docker Desktop Before Apple `container`

## Status

Accepted (2026-07-22; operator direction in agentic-sandbox#438)

## Context

Agentic Sandbox already exposes three execution kinds through the management
boundary: `host`, `docker`, and `qemu`. The host supervisor and Docker lifecycle
paths are implemented on Linux, but `agentic-mgmt` cannot yet build or start on
Apple Silicon because Linux VM dependencies and libvirt monitoring are coupled
to the management binary. Published agent images are also runner-native rather
than guaranteed multi-platform images.

The earlier Apple plan made Apple `container` the first supported macOS runtime.
That provides a desirable VM-backed isolation option, but it requires a new
provider contract and currently targets a narrower Apple platform. It need not
block a useful Apple release based on the existing host and Docker contracts.

The first Apple release needs to:

- run the management control plane natively on Apple Silicon;
- execute explicitly selected native-host instances;
- execute Linux/arm64 agent images through Docker Desktop;
- make Linux-only VM, VFIO, cgroup, namespace, and seccomp capabilities
  unavailable rather than emulated or silently weakened;
- build and smoke-test on mutsu, not teroknor;
- preserve Linux libvirt, Cloud Hypervisor, host, and Docker behavior.

## Decision

Deliver Apple Silicon support in three phases.

1. **Foundation**: make `agentic-mgmt` and the host daemon build natively for
   `aarch64-apple-darwin`, feature-gate or target-gate Linux VM dependencies,
   publish architecture-correct `linux/amd64` plus `linux/arm64` agent images,
   and add backward-compatible discovery for host, Docker, and VM runtime
   availability.
2. **Initial providers**: support the existing `runtime: host` contract on
   macOS using per-user paths and launchd, and support the existing
   `runtime: docker` contract through Docker Desktop.
3. **Validation and release**: compile and run credential-free host and Docker
   smoke checks on mutsu, then expand the signed/notarized package and promote
   the Apple release lane.

Apple `container` remains an explicit, separate VM-backed provider. Its spike
and implementation continue under #488 and #489 after the initial host and
Docker release; they do not block that release.

Provider choice remains explicit. Host and Docker keep their existing public
runtime values, so no breaking API or schema change is expected. Capability and
availability additions must be additive and synchronized across OpenAPI, CLI,
dashboard, and runtime metadata.

## Consequences

### Positive

- Reuses completed host-supervisor and Docker lifecycle work.
- Produces a useful Apple development runtime without waiting for a new
  upstream provider.
- Preserves a clear isolation ladder: host is full host access, Docker Desktop
  is shared-kernel container isolation, and Apple `container` is a later
  VM-backed option.
- Makes arm64 OCI delivery reusable by Docker Desktop and Apple `container`.
- Keeps unsupported Linux VM and GPU behavior explicit and fail-closed.

### Negative

- The first Apple release does not provide the VM-grade isolation of the Linux
  KVM path or the planned Apple `container` path.
- Docker Desktop has distinct networking and bind-mount semantics that require
  platform-specific validation and documentation.
- Native host execution has no cgroups, namespaces, or seccomp boundary and
  must remain opt-in with a prominent full-host-access warning.
- The project must maintain a serialized Apple validation lane in addition to
  Titan Linux CI.

## Migration and Rollback

The change is additive. Linux defaults do not change. Darwin support is enabled
only when the target-specific build, provider smoke, and packaging gates pass.
Any failing Apple provider can be removed from the advertised availability set
or disabled without changing Linux behavior or the public runtime values.

## Issue Plan

- #667 — Darwin management and host-daemon build foundation.
- #668 — multi-platform agent OCI images.
- #669 — native-host support on Apple Silicon macOS.
- #670 — Docker Desktop support on Apple Silicon macOS.
- #671 — mutsu build and provider smoke-validation lane.
- #672 — additive cross-runtime availability and capability discovery.
- #462 — signed/notarized package expansion and production release promotion.
- #488 and #489 — later Apple `container` spike and provider implementation.
- #495 — macOS Keychain backend before production workstation secret-storage
  promotion.

## References

- #438 — Apple host-support umbrella.
- #460 through #474 — existing host runtime and session foundations.
- #478 — public runtime image publication.
- #481 and #664 — Apple package and signing contract.
- #621 and #622 — container isolation findings that remain applicable.

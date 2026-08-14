# Operating profile

Agentic Sandbox is a self-hosted runtime for persistent AI agent processes. It
keeps execution, data, and operator control inside infrastructure you manage.

## Deployment and runtime model

| Axis | Agentic Sandbox |
|------|-----------------|
| **Hosting model** | Self-host only. No hosted control plane. |
| **Isolation boundary** | KVM hypervisor (per-agent kernel). Rootless containers as a lighter alternative. |
| **Session duration** | Designed for sessions measured in hours to days. |
| **Data path** | All traffic stays on the operator's network. |
| **Agent shape** | Bring-your-own agent. Claude Code is the primary tested agent; the runtime is agent-agnostic. |
| **Orchestration** | Single-host today. Multi-host and Kubernetes are on the roadmap. |

## Security claim boundaries

Use the dated [security status](security/security-status.md) page as the public
source of truth for launch claims.

- Claim self-hosted, local-first operation and KVM isolation as runtime
  capabilities.
- Describe agent transport identity as support for UDS, vsock, and mTLS; do
  not imply every deployment profile is verified unless the release evidence
  says so.
- Describe credentials as metadata-first and lease-oriented; do not claim
  absolute zero credential exposure.
- Describe standards work as alignment; do not claim certification or
  compliance without a real program and evidence.
- Claim signed artifacts, SBOMs, and image provenance only for releases where
  those artifacts are attached and verified.

## Intended workloads

- Agent workloads where source code or data cannot leave the network (regulated industries, on-prem, air-gapped).
- Long-running sessions that need durable terminal and process state.
- Workloads where hypervisor-level isolation is required (untrusted code execution, adversarial workloads).
- Internal platforms running agent workloads for multiple users on shared infrastructure.

## Operational considerations

- Operators provide and maintain the Linux hosts, KVM/libvirt stack, storage,
  networking, and release lifecycle.
- Rootless containers provide the lighter runtime tier; KVM-backed virtual
  machines provide the stronger isolation tier.
- Multi-host orchestration and Kubernetes integration remain roadmap work.
- Public security claims remain bounded by the evidence linked from the
  [security status](security/security-status.md) page.

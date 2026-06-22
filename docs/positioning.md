# Positioning

Where Agentic Sandbox fits relative to other agent runtimes — described by capability axis, not by vendor.

## Design axes

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

## When this is a good fit

- Agent workloads where source code or data cannot leave the network (regulated industries, on-prem, air-gapped).
- Long-running sessions where the bottleneck is "did the terminal stay open" rather than cold-start latency.
- Workloads where hypervisor-level isolation is required (untrusted code execution, adversarial workloads).
- Internal platforms running agent workloads for multiple users on shared infrastructure.

## When something else is a better fit

- Short, transactional sandboxes for individual tool calls — a hosted microVM service will be cheaper and faster to start.
- Container-based dev-environment-as-a-service workflows — use a dedicated dev-environment platform.
- Turnkey hosted "agent as a product" with no self-hosting — pick a managed offering.

The shared theme: if the operational cost of running this yourself isn't justified by the data, isolation, or session-length requirements, a hosted service will be simpler.

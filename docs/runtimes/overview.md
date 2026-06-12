# Runtime Map

Agentic Sandbox supports two execution substrates: full KVM virtual machines
for strong isolation, and Docker-backed containers for fast local validation.

## VM vs Container

| Runtime | Best fit | Tradeoff |
| --- | --- | --- |
| **KVM VM** | Long-running autonomous agents that need kernel isolation, controlled networking, loadouts, and per-VM secrets. | Slower provisioning and more host prerequisites. |
| **Container Runtime** | Fast local validation, dashboard testing, and low-friction agent process management without KVM setup. | Weaker isolation boundary than a VM. |

## Runtime Foundations

- [Platform Support](../platform-support.md) - supported hosts, guests,
  hypervisors, and build targets.
- [Runtime Parity](../runtime-parity.md) - VM and container behavior alignment.
- [Container Runtime](../container-runtime.md) - Docker-backed instances and
  dashboard/CLI surfaces.
- [VM Lifecycle](../vm-lifecycle.md) - VM state machine, provisioning, and
  teardown.

## Provisioning And Storage

- [Deployment](#/DEPLOYMENT) - host setup and production deployment.
- [Loadouts](#/LOADOUTS) - YAML loadouts and profile composition.
- [Agent Share](../agentshare.md) - virtiofs shared storage layout and data
  flows.
- [Security: Resource Quota Design](../security/resource-quota-design.md) -
  CPU, memory, disk, and quota strategy.

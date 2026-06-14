# Runtime Map

Agentic Sandbox models three execution substrates: local host processes,
Docker-backed containers, and full KVM virtual machines. Operators choose the
isolation tier per instance.

## Runtime Spectrum

| Runtime | Best fit | Tradeoff |
| --- | --- | --- |
| **Host Runtime** | AIWG base-level local execution where the agent runs directly on the user's host. | Least isolation: the agent has full host access unless separately constrained by the OS. Durable execution requires the host supervisor tracked by #460. |
| **KVM VM** | Long-running autonomous agents that need kernel isolation, controlled networking, loadouts, and per-VM secrets. | Slower provisioning and more host prerequisites. |
| **Container Runtime** | Fast local validation, dashboard testing, and low-friction agent process management without KVM setup. | Weaker isolation boundary than a VM. |

## Runtime Foundations

- [Platform Support](../platform-support.md) - supported hosts, guests,
  hypervisors, and build targets.
- [Runtime Parity](../runtime-parity.md) - VM and container behavior alignment.
- [Container Runtime](../container-runtime.md) - Docker-backed instances and
  dashboard/CLI surfaces.
- [Host Runtime Supervisor](./host-supervisor.md) - durable bare-host process,
  PTY/session, reattach, and multi-watch-agent boundary.
- [VM Lifecycle](../vm-lifecycle.md) - VM state machine, provisioning, and
  teardown.

## Provisioning And Storage

- [Deployment](#/DEPLOYMENT) - host setup and production deployment.
- [Loadouts](#/LOADOUTS) - YAML loadouts and profile composition.
- [Agent Share](../agentshare.md) - virtiofs shared storage layout and data
  flows.
- [Security: Resource Quota Design](../security/resource-quota-design.md) -
  CPU, memory, disk, and quota strategy.

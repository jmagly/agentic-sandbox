# Operations Map

Use this section after the first local run works. It groups the material needed
to deploy, monitor, troubleshoot, and harden a sandbox fleet.

## Operator Lanes

| Lane | What it covers | Read |
| --- | --- | --- |
| **Production Setup** | Host prerequisites, service setup, deployment layout, and AIWG-connected mode. | [Deployment](#/DEPLOYMENT) |
| **Day-2 Procedures** | Server lifecycle, runtime management, task operations, HITL, and incident routines. | [Operations](#/OPERATIONS) |
| **Monitoring** | Prometheus, Grafana, metrics naming, SLOs, alerts, and dashboard wiring. | [Monitoring](../monitoring.md) |
| **Troubleshooting** | Common install, runtime, task, agent, and AIWG integration failures. | [Troubleshooting](#/TROUBLESHOOTING) |

## Operations Flow

1. [Deployment](#/DEPLOYMENT) - install and configure the host.
2. [Operations](#/OPERATIONS) - run the service day to day.
3. [Monitoring](../monitoring.md) - instrument the fleet.
4. [Reliability Map](../reliability/overview.md) - define SLOs and failure
   handling.
5. [Troubleshooting](#/TROUBLESHOOTING) - diagnose and recover.

## Specialized Ops

- [Crash Loop Detection](../crash-loop.md) - runtime crash detection and
  operator unblock.
- [Telemetry](../telemetry.md) - metrics and textfile collector pipeline.
- [Transport Audit](../transport-audit.md) - operator-facing event/log streams.
- [Observability Design](../observability/README.md) - deeper observability
  architecture and implementation checklist.

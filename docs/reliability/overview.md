# Reliability Map

Reliability docs explain how the sandbox fleet should behave when runtimes
fail, transports flap, sessions outlive processes, or host capacity changes.

## Reliability Lanes

| Lane | What it covers | Read |
| --- | --- | --- |
| **Overview** | Entry point and role-based reading paths. | [Reliability Overview](../reliability-README.md) |
| **Quickstart** | First reliability checks and baselines. | [Reliability Quickstart](../reliability-quickstart.md) |
| **Architecture** | Visual flows, failure domains, and component relationships. | [Reliability Architecture](../reliability-architecture.md) |
| **Design** | Full technical design for implementation and review. | [Reliability Design](../reliability-design.md) |

## Reliability Reading Path

1. [Reliability Overview](../reliability-README.md)
2. [Reliability Quickstart](../reliability-quickstart.md)
3. [Reliability Design Summary](../reliability-design-summary.md)
4. [Reliability Architecture](../reliability-architecture.md)
5. [Reliability Design](../reliability-design.md)
6. [Reliability Implementation Checklist](../reliability-implementation-checklist.md)

## Related Operations Docs

- [Crash Loop Detection](../crash-loop.md)
- [Telemetry](../telemetry.md)
- [Transport Audit](../transport-audit.md)
- [Session Reconciliation](#/SESSION_RECONCILIATION)

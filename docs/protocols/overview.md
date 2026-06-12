# Protocol Map

Agentic Sandbox exposes three families of interfaces: local operator APIs,
agent/runtime control streams, and integration contracts for AIWG and task
orchestration.

## Protocol Lanes

| Lane | What it covers | Read |
| --- | --- | --- |
| **HTTP REST** | Agents, sessions, tasks, containers, logs, and operator actions. | [API](#/API) |
| **WebSocket** | Dashboard events, PTY output, metrics updates, and session attach flows. | [WebSocket Protocol](../ws-protocol.md) |
| **Tasks** | Task manifests, lifecycle states, wait semantics, and implementation patterns. | [Task Orchestration API](../task-orchestration-api.md) |
| **AIWG** | Register with `aiwg serve`, accept mission dispatch, and stream mission events. | [AIWG Executor Contract](../aiwg-executor.md) |

## API And Protocol References

- [API](#/API) - complete REST API reference.
- [WebSocket Protocol](../ws-protocol.md) - live dashboard and event streams.
- [Task Orchestration API](../task-orchestration-api.md) - task submission and
  monitoring.
- [Task API Implementation Guide](../task-api-implementation-guide.md) -
  integration patterns.
- [Task API Quick Reference](../task-api-quick-reference.md) - curl-ready
  examples.
- [Task Run Lifecycle](../task-run-lifecycle.md) - state transitions.
- [Session Reconciliation](#/SESSION_RECONCILIATION) - recovery behavior.
- [v2 Migration Guide](../v2-migration-guide.md) - move to the A2A-aligned v2
  surface.
- [CLI Design](../cli-design.md) - `sandboxctl` command design.

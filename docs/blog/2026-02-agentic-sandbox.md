# February 2026: VM control and session repair mature

**Published:** 2026-02-28  
**Project:** Agentic Sandbox  
**Window:** February 2026  
**Public release covered:** None. This was pre-release foundation work.

Agentic Sandbox is a runtime for AI agents that need a real shell, files, and
state. It keeps that work inside a user-controlled sandbox instead of
letting the agent operate directly on the host.

February focused on making that sandbox easier to run. VM life cycle controls,
session repair, health data, scripts, and dashboard views all moved forward.

## TL;DR

February made the early system more usable. Users gained better VM controls,
better session view, and stronger repair behavior when agents reconnect.
The project also added deployment and chaos-test scripts so failures could be
tested instead of guessed at. There was still no public release, but the system
was moving from prototype toward a user workflow.

## What this means

- VMs became easier to control.
- Lost sessions became easier to repair.
- Failures became easier to test.
- State became clearer.
- Cleanup became safer.

## By the numbers

| Public surface | Current state |
| --- | --- |
| Runtime direction | VM-backed agent workspaces with managed life cycle controls |
| Team interface | Dashboard controls, HTTP API routes, CLI groundwork |
| Uptime focus | Session sync, stale connection cleanup, and heartbeat handling |
| Source | `https://github.com/jmagly/agentic-sandbox` |
| Public release | None this month |

## Highlights

### VM controls move into the product

February added stronger VM life cycle paths. Users could list, inspect,
start, stop, destroy, create, delete, and restart sandboxes through the
control surface.

This matters because a VM sandbox is only useful if it is easy to operate. You
should not have to remember every low-level libvirt command to recover a stuck
agent or clean up a test run.

### Sessions can reconcile after reconnect

The agent and server gained a sync protocol. In plain terms, when an
agent reconnects, both sides can agree on which sessions should stay alive and
which ones should be cleaned up.

That helps with real-world agent runs. Network paths break, servers restart, and
VMs reboot. The product needs a way to repair state after those events.

### Telemetry becomes more useful

Metrics and logs gained more session detail. Users could see more about
what agents were doing, how sessions behaved, and where the runtime was stuck.

Good health data saves time. Instead of asking "is the agent dead?", you can check
which part of the path is still healthy.

### Deployment and chaos scripts appear

The project added scripts for deployment, setup, and chaos testing. Chaos
tests deliberately create bad conditions so repair paths can be checked.

For a sandbox platform, that is a serious step. The hardest bugs often happen
when a VM disappears, a connection stalls, or setup fails halfway through.

## Features shipped

### VM life cycle API and dashboard work

The server added routes and dashboard controls for day-to-day VM
actions. The work included progress tracking, safe retries, checks, and rate
limits.

Safe retry means a repeated request can be handled safely. That is important in
user tools because browsers, scripts, and networks sometimes retry.

### Session sync

The protocol gained messages for reconnect, report, keep, and kill flows. The
server and agent could use those messages to repair their shared view of active
sessions.

If you have a long-running terminal or task, this keeps reconnect behavior from
turning into duplicate state or orphaned sessions.

### Dashboard view

The UI added more session events and a sessions panel. Users could see
session state in the browser and act on it more directly.

That keeps the dashboard aligned with how the product is used: watch an agent,
inspect its runtime, and intervene when needed.

### Uptime modules

The agent and server gained uptime, metrics, and runner modules.
Heartbeat timeout and stale connection detection made the system less likely to
wait forever on a dead path.

## Fixes

February fixed VM shutdown behavior in the UI so stop and force-kill paths
removed VMs more completely. Setup also gained retry behavior around
cloud-init and network setup.

Those fixes addressed common user pain: a VM that looks stopped but leaves
state behind, or a first boot that fails because the network was not ready yet.

## Performance & uptime

Uptime was the main theme. Heartbeats, stale connection detection,
work tracking, checks, and chaos scripts all made the runtime more
defensive.

The system still needed more hardening, but it now had explicit tools for
finding and recovering from bad states.

## Breaking changes and migrations

None this month. The project had no public release or stable migration promise
yet.

## Releases

None this month. February was pre-release build work.

## Packages & safety

Package manifests changed as the Rust server and agent grew.
Safety work continued through server checks, rate limits, and stricter VM
control behavior.

No public package or registry release shipped this month.

## Docs & builder experience

Operational and deployment docs expanded. The repo added build
notes, life cycle references, and setup scripts that made the project easier to
run outside the original build work path.

## Tests & CI

The project continued to grow Rust and end-to-end coverage around VM control,
agent behavior, and session sync. The important shift was coverage for
repair paths, not only happy paths.

## Cross-project impact

The sync work made Agentic Sandbox a better base for higher-level
agent workflows. A workflow system can only trust a runtime if session state is
easy to repair after reconnects and restarts.

## Known issues & open threads

- The public release train had not started.
- Session long life still needed more real-world soak testing.
- VM setup still depended on host setup details that later work would
  smooth out.

## What's next

The next useful step was to make the runtime surface more complete: stronger
CLI commands, better loadout profiles, and cleaner terminal/session behavior.

## Links

- [Operations](../operations/overview.md)
- [VM life cycle](../vm-lifecycle.md)
- [Session sync](../SESSION_RECONCILIATION.md)
- [Uptime](../reliability/overview.md)

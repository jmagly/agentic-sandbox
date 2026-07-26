# April 2026: Loadouts, local control, and live terminal hardening

**Published:** 2026-04-30  
**Project:** Agentic Sandbox  
**Window:** April 2026  
**Public release covered:** None. This was pre-release build work.

Agentic Sandbox is built for agents that need a real runtime. They can work in a
container or VM-like workspace, use a terminal, and keep files while users
keep the boundary and life cycle under control.

April was the month where that control surface became much more complete. The
project added loadout profiles, a stronger CLI, local admin access, token
reloads, session replay improvements, and more reliable terminal sharing.

## TL;DR

April turned the platform into something closer to a user tool. Loadouts
made sandbox images more repeatable. The CLI gained read and write verbs for
VMs, agents, sessions, tasks, events, loadouts, health, and ops. Terminal
reconnects became safer through replay, keyframes, and shared PTY handling.
There was no public release yet, but many pieces that later shipped in May were
built during this month.

## What this means

- Teams could shape a sandbox.
- The CLI could do real work.
- The browser could rejoin sessions.
- Terminal state became less fragile.
- Local tools got safer access.
- Tokens could reload cleanly.
- HITL prompts got an API.
- Loadouts became data.
- Containers joined the plan.
- Replay made reconnects clearer.
- Setup became more steady.
- May had stronger pieces ready.
- The shell felt less brittle.
- The API had clearer jobs.
- Local access got tighter checks.
- Users had less guesswork.

## By the numbers

| Public surface | Current state |
| --- | --- |
| Runtime settings | Composable loadout layers and profiles |
| Team access | Dashboard, HTTP API, CLI, WebSocket/SSE streams |
| Admin access direction | Bearer tokens, local socket checks, and token reload |
| Source | `https://github.com/jmagly/agentic-sandbox` |
| Public release | None this month |

## Highlights

### Loadouts make sandboxes repeatable

Loadouts define what goes into a sandbox image. April added the model for
layers, profiles, and provider settings.

This helps because agents often need more than a blank machine. One run may
need browser tools. Another may need research tooling. A loadout lets users
choose a prepared shape instead of rebuilding each VM by hand.

### The CLI becomes a user tool

The command-line interface gained a real structure. It added contexts, an HTTP
client, output formatting, audit logging, read-only commands, and mutating
commands.

That gives users a scriptable path. The dashboard is useful for watching,
but a CLI is what you use in scripts, smoke checks, and repair scripts.

### Terminal sharing gets safer

April improved how terminal sessions are shared and replayed. The system moved
toward one PTY per session, with multiple browser clients attaching to the same
state. It also added replay and keyframe behavior so reconnects do not show a
blank terminal.

If you close a browser tab and come back, you want the terminal to make sense.
You do not want to lose the screen or by mistake start a duplicate shell.

### Local admin access gets stricter

The control API added local socket ID checks and token reload behavior. A local
socket stays on the host. It lets the server check which local process is
connecting.

This is a good fit for single-host work. It lets local tools act with
stronger ID while keeping remote access behind explicit tokens.

## Features shipped

### Loadout model and registry

The repo added loadout layers for base dev work, minimal work, Docker,
network tools, and health checks. It also added profiles and provider
settings, plus manifest resolution and cloud-init build.

In plain terms, this lets the project define "what kind of sandbox do I want?"
as data. That data can feed VM setup, the dashboard, and later release docs.

### API and local admin surfaces

April expanded the HTTP API around agentshare, sessions, events, AIWG companion
endpoints, HITL prompts, and loadouts. HITL means "human in the loop": a task
can pause and ask a person for input instead of guessing.

The API also gained token reload and local local ID access. Those pieces
made the server safer to operate without constant restarts.

### CLI command expansion

The CLI added commands for attaching to sessions, running commands, checking
health, managing HITL flows, viewing loadouts, and handling life cycle actions.

For a user, this means less dependence on browser workflows. You can
inspect and drive the runtime from scripts.

### Live terminal and session replay

The terminal stack added multicast behavior, auto join on attach, keyframe
injection, step-by-step replay, resize fixes, and reconnect support.

This is the difference between "we stream bytes" and "we can operate a live
terminal as a shared product feature."

### Container runtime groundwork

Late April added container life cycle endpoints and an container agent base
image path with PTY exec support. That gave the project a faster runtime option
next to the stronger VM boundary.

## Fixes

April fixed many visible terminal and setup problems:

- reconnects could now request history instead of showing a blank terminal,
- resize corruption was reduced when more than one client attached,
- stale file descriptors and session routing mismatches were cleaned up,
- shell paths and welcome screens were made more consistent, and
- loadout setup became more resilient.

These fixes all serve the same goal: make a sandbox feel steady when an
user is watching and driving it.

## Performance & uptime

Session replay and terminal buffering improved. Raw bytes could be stored in the
replay buffer and encoded at the wire edge, which keeps hot terminal paths more
efficient.

The server also moved blocking libvirt calls away from the async runtime and
added watchdog behavior. That helps prevent slow VM works from freezing
unrelated API work.

## Breaking changes and migrations

None for public users. There was still no public release or stable API promise
for April.

## Releases

None this month.

## Packages & safety

April safety work included token gating, admin checks, token reload,
and local local ID access. These were design and build steps
toward safer work before public release.

Package manifests changed as the CLI, server, and gateway evolved,
but no public package release shipped this month.

## Docs & builder experience

The project added protocol references, loadout examples, CLI behavior, and
user-facing settings. The docs also explained the WebSocket transport
recipe used by the sandbox.

Builder experience improved through completions, watch mode, audit logs, and
more direct CLI output.

## Tests & CI

April added and updated tests around sessions, HTTP/API behavior, loadouts, and
terminal handling. Formatting and CI fixes kept the Rust codebase aligned as the
surface area grew.

## Cross-project impact

Loadouts and HITL support made Agentic Sandbox more useful for AIWG workflows.
Those workflows need prepared workspaces and safe points where a person can
approve or guide work.

## Known issues & open threads

- The public release train had not started.
- Provider install parity still needed more proof.
- Terminal replay and multi-client behavior needed more load testing.
- Container and VM runtime parity was still in progress.

## What's next

May would turn much of this foundation into public releases: container parity,
the v2 runtime surface, release gates, safety hardening, and stronger
docs.

## Links

- [Loadouts](../LOADOUTS.md)
- [Container runtime](../container-runtime.md)
- [WebSocket protocol](../ws-protocol.md)
- [CLI design](../cli-design.md)

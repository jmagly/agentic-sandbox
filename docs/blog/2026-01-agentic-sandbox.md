# January 2026: Agentic Sandbox takes shape

**Published:** 2026-01-31  
**Project:** Agentic Sandbox  
**Window:** January 2026  
**Public release covered:** None. This was pre-release foundation work.

Agentic Sandbox gives AI agents a controlled place to do real work. The goal is
simple: let an agent run commands, keep files, and use a terminal while the host
keeps a clear safety boundary around it.

January was the inception month. The project moved from product shape and
design notes into a working base: a Rust server, a Rust agent,
a command-line tool, VM image build scripts, shared storage, and the first web
dashboard.

## TL;DR

January built the starting point for the whole system. The project defined the
runtime isolation model, added a control plane, and gave agents a path to
connect back from a sandbox. It also set up early VM setup, a dashboard,
and end-to-end checks. There was no public release yet, so this month is best
read as the foundation before packaging and release work.

## What this means

- Agents got a safer place to work.
- The host stayed outside the test.
- The first server could track agents.
- The first CLI could drive runs.
- The first dashboard could show state.
- VM setup became a checked-in path.
- Tests made the base less fragile.
- Release work could build on this.
- The goal stayed clear.
- Work stayed inside the sandbox.

## By the numbers

| Public surface | Current state |
| --- | --- |
| Runtime direction | QEMU/KVM virtual machines with shared storage |
| Main components | Server, agent client, CLI, dashboard, and image tooling |
| Team interface | HTTP dashboard, gRPC control stream, WebSocket output stream |
| Source | `https://github.com/jmagly/agentic-sandbox` |
| Public release | None this month |

## Highlights

### A clear isolation model

The project started with a strong rule: agents should be useful without getting
free access to the host. The early design work chose a VM-backed runtime
path, shared folders through virtiofs, and a server that brokers what
the agent can do.

That matters because agents are not just chat tools here. They can run code and
touch files. A real sandbox lets you hand them work without treating the host as
part of the experiment.

### A server users can build on

January added the Rust server. It is the control point for agent
registration, commands, logs, metrics, and WebSocket output. It also introduced
the early dashboard so a user could see agents and session output without
tailing logs by hand.

If you are running an agent, this gives you one place to ask, "What is alive,
what is it doing, and how do I reach it?"

### A native Rust agent path

The Rust agent client became the supported agent path. It can connect to the
server, stream logs from the shared workspace, and participate in the
control protocol.

This is important because the agent process is the code that runs inside the
sandbox. It needs to be small, direct, and easy to package into VM images and
service managers.

### VM setup groundwork

The month added scripts and settings for building QEMU base images and
setup agent VMs. It also added host lockdown work, firewall setup, and
shared-folder wiring.

The VM path was still young, but the shape was already visible: build a base
image, boot an agent workspace, mount the shared area, and let the agent check
in.

### Terminal access becomes part of the product

The dashboard gained an xterm.js terminal view and the backend gained PTY
support. A PTY is a real terminal stream. It lets a browser show the same kind
of live shell you would see in a local terminal.

For an agent sandbox, that is useful for debugging. You can watch setup, inspect
state, and recover from simple mistakes without rebuilding the whole runtime.

## Features shipped

### Runtime and setup

January introduced QEMU image build base system, VM setup scripts, and
the first agent-ready profiles. The setup path prepared a build work
workspace inside the guest, wired shared folders, and set up network access
back to the server.

The practical goal was to make a sandbox repeatable. Instead of hand-editing a
machine, a user could use checked-in scripts and profiles to rebuild the
same starting point.

### Control and dashboard

The server added gRPC control, HTTP dashboard routes, WebSocket
streaming, metrics, and early task/run life cycle design. The dashboard could
show agent state and stream terminal output.

This made the project more than a VM script collection. It became a control
plane for many agent sandboxes.

### CLI and packaging

The command-line tool started to take shape, and the agent gained command-line
flags for connection settings. The project also added Docker Compose support,
systemd units, install scripts, and cloud-init templates.

These pieces are not flashy, but they are what make the system operable. A
sandbox that only works from a builder shell is not enough.

### Session and coordination foundation

The project added the early session model, checkpointing, retries, timeouts,
health checks, cleanup, and audit hooks. In plain terms, this is the machinery
that helps long-running agent work survive common failures.

If an agent disconnects, a command hangs, or a VM needs cleanup, the system
needs a known path instead of leaving mystery state behind.

## Fixes

Terminal and setup fixes landed throughout the month. The work tightened
terminal sizing, made the dashboard clear action behave like a terminal command,
fixed shared-folder mounts, and adjusted workspace paths inside the VM.

The welcome screen and prompt handling also improved. These small fixes matter
because the terminal is one of the main ways a user understands what the
agent runtime is doing.

## Performance & uptime

January added the first uptime model: heartbeat timeouts, retry handling,
cleanup routines, circuit-breaker ideas, and health reporting. It also added
structured trace IDs so events can be tied together when a run spans several
services.

This work set the direction for later months. A sandbox must fail in ways that
users can see and recover from.

## Breaking changes and migrations

None this month. The project had no public release or stable migration promise
yet.

## Releases

None this month. January was pre-release build work.

## Packages & safety

Safety work started early. The runtime design used VM isolation, firewall
lockdown, seccomp planning, and tighter host access rules as first-class parts
of the product.

Package files for Rust, Go, and Python changed as the project took shape, but
there were no public packages to publish or upgrade yet.

## Docs & builder experience

The project added design notes, life cycle docs, task API design, safety
settings, deployment scaffolding, and a quick path for local build work.

The docs were still aimed at builders and users rather than end users, which
fits the month: the product was forming its base.

## Tests & CI

End-to-end checks and Rust tests were introduced with the first runtime and
coordination paths. The tests covered packaging, agent connection, and core
control behavior.

## Cross-project impact

Agentic Sandbox became the runtime layer for broader AIWG-style agent work. It
gave those workflows a place to run commands and keep state without assuming the
builder's host was the agent's workspace.

## Known issues & open threads

- QEMU and libvirt networking still needed more hardening.
- The VM path needed more repeatable setup and cleanup.
- OpenTelemetry and secret-manager link were still future work.
- The public release process had not started yet.

## What's next

The next step was to make VM life cycle control stronger, make sessions survive
reconnects more cleanly, and turn the early control plane into something an
user could use day to day.

## Links

- [Design overview](../architecture/overview.md)
- [VM life cycle](../vm-lifecycle.md)
- [Task run life cycle](../task-run-lifecycle.md)
- [Security status](../security/security-status.md)

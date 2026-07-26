# May 2026: v2 contracts, container parity, and release hardening

**Published:** 2026-05-31  
**Project:** Agentic Sandbox  
**Window:** May 2026  
**Public releases covered:** [v2026.5.0](../releases/v2026.5.1.md),
[v2026.5.1](../releases/v2026.5.1.md), [v2026.5.2](../releases/v2026.5.2.md),
[v2026.5.3](../releases/v2026.5.3.md), [v2026.5.4](../releases/v2026.5.4.md),
[v2026.5.5](../releases/v2026.5.5.md), [v2026.5.6](../releases/v2026.5.6.md),
[v2026.5.7](../releases/v2026.5.7.md), [v2026.5.8](../releases/v2026.5.8.md),
[v2026.5.9](../releases/v2026.5.9.md), [v2026.5.10](../releases/v2026.5.10.md),
[v2026.5.11](../releases/v2026.5.11.md), [v2026.5.12](../releases/v2026.5.12.md),
[v2026.5.13](../releases/v2026.5.13.md), [v2026.5.14](../releases/v2026.5.14.md),
[v2026.5.15](../releases/v2026.5.15.md), [v2026.5.16](../releases/v2026.5.16.md),
and [v2026.5.17](../releases/v2026.5.17.md)

Agentic Sandbox gives agents a real execution workspace while users keep
control over runtime ID, life cycle, terminal access, and release
provenance.

May was the first public release month. It delivered the container runtime
baseline, the v2 runtime contract, stronger PTY and dashboard behavior,
safety fixes, supply-chain hardening, and release gates that made later
release safer.

## TL;DR

May turned Agentic Sandbox from a fast-moving pre-release project into a tagged
public runtime. Users got container and VM runtime choices, a dashboard that
understands both, a v2 runtime API, AgentCard lookup, PTY-over-WebSocket
binding, and a conformance path. The project also tightened secrets, cloud-init
files, Docker access, CI pinning, and VM image checks. The end of the
month was mostly about release hardening so release would fail closed when
the VM substrate or runner lane was not healthy.

## What this means

- Users got tagged builds.
- Containers became a real path.
- VMs stayed the strong path.
- The dashboard showed both paths.
- The API got clearer shape.
- Tasks got stable routes.
- Agent cards described each runtime.
- Terminal access became documented.
- Human prompts had a path.
- Push settings had routes.
- Old routes still worked.
- New routes were easier to find.
- Secrets got more care.
- Docker access was narrowed.
- Logs hid bearer tokens.
- VM files used tighter modes.
- CI used pinned inputs.
- Image checks got stricter.
- Bad releases could stop early.
- Release notes told the story.
- Docs matched more of the code.
- The CLI covered more jobs.
- Test gates became stronger.
- Browser QA got a profile.
- The runtime was easier to prove.
- Teams had a clearer upgrade path.

## By the numbers

| Public surface | Current state |
| --- | --- |
| Runtime choices | Container and QEMU/KVM VM paths |
| Public contract | v2 runtime API with admin, per-instance, and health surfaces |
| Terminal access | PTY WebSocket binding for live sessions |
| Published version series | `2026.5.x` CalVer releases |
| Docs | `https://docs.aiwg.io/agentic-sandbox/` |
| Source | `https://github.com/jmagly/agentic-sandbox` |

## Highlights

### Containers join VMs as a first-class runtime

May added container runtime parity with the VM dev profile. Users
could create an agent container from the dashboard, choose an image, and get a
tooling-rich workspace without waiting for a full VM boot.

Use the container path when you want speed and reuse. Use the VM path
when you need a stronger boundary. The product now supports both directions.

### The v2 runtime contract becomes real

The project shipped the v2 runtime surface. It separates admin actions,
per-agent task routes, and health checks routes so each part has a clearer job.

This matters for links. A client can send messages, list tasks, subscribe
to task updates, and discover an agent's signed AgentCard without relying on
private dashboard behavior.

### AgentCard lookup gives each runtime an ID

AgentCard lookup lets a client ask a running sandbox what it supports. The
card is signed and describes interfaces, safety schemes, and extensions.

For a tool builder, this is a cleaner handshake. Instead of guessing which
routes exist, the client can read what the runtime says it supports.

### Live terminal work moves into the v2 path

The PTY WebSocket binding gives live terminal sessions a documented
route under the per-instance surface. Dashboard work moved toward that binding,
with role and membership information for attached clients.

That makes terminal access less like a side channel and more like a supported
runtime feature.

### Release gates get stricter

May exposed a hard truth: a release is only as good as its checks lane. The
project tightened tag gates, VM E2E checks, image checks, runner
selection, release attachments, and docs sync.

That work is not glamorous, but it protects users. If a VM image is bad or a
runner cannot prove the release, release should stop.

## Features shipped

### Container runtime and image catalog

The project added a shared dev base image for agents and container
variants for common agent providers. The dashboard gained a runtime dropdown,
container image picker, and container-specific controls.

This gives users a fast path for agent work. A container can start quickly,
run the same agent client model, and still connect back through the control
server.

### Unified instance dashboard

The dashboard began treating VMs and containers as instances in one place. Rows
show the runtime type, and controls map to what each runtime can actually do.

That avoids a common user mistake: showing a button that looks useful but
does not match the underlying runtime.

### v2 runtime API

The v2 runtime work added task message routes, task state reads, cancellation,
server-sent task updates, push notification settings, AgentCard reads, and
admin setup routes.

It also added a migration path for older v1 clients. v1 routes still worked, but
responses pointed clients toward the successor version.

### A2A-compatible extension model

The runtime declared extensions for runtime data, safe retry, human prompts,
multi-tenant shape, adapter commands, and PTY behavior. The names are technical,
but the value is straightforward: clients can know which extra behaviors are
available instead of relying on hidden assumptions.

### Conformance harness

A separate conformance harness was added so the runtime contract could be
tested from the outside. This is important because an API contract should be
checked like a product surface, not only as internal unit tests.

### Browser QA and scripts loadouts

May added browser QA profiles and control loadouts. These profiles
prepared the sandbox for browser-driven testing and controlled scripts work.

For agent teams, this means a sandbox can be shaped for browser tasks instead
of starting from a generic shell each time.

## Fixes

### Terminal and session fixes

May fixed resize handling, terminal replay bounds, transcript archive behavior,
dashboard reset behavior, and session attach data.

The visible effect is simple: live terminal sessions are less likely to
look blank, corrupt, or stale when a browser reconnects.

### VM and setup fixes

The VM path gained stronger readiness checks, SSH wait diagnostics, UEFI boot
support, image sanity checks, compressed qcow2 handling, and setup
cleanup.

These fixes make VM-backed agents less fragile. If setup fails, the scripts
should tell you why. If the base image is wrong, the verifier should stop before
trusting it.

### API and runtime fixes

The runtime fixed task routing, instance-scoped task lists, task artifact
access, task state transitions, and Docker-backed v2 instance creation.

These fixes are important for clients that call the API directly. A task route
should always address the intended runtime, not leak across instances.

## Performance & uptime

May added hot event memory metrics, event archive behavior, bounded PTY replay,
TUI screen-state stabilization, release runner hardening, and more diagnostic
output around VM setup.

The system also became more fail-closed around releases. Tag release depends
on release-blocking checks instead of letting artifacts publish after a bad
VM gate.

## Breaking changes and migrations

The legacy Python SDK and old Python agent runtime were removed. The supported
paths are the Rust CLI, the REST API, and the Rust agent client.

For API users, v1 routes still responded, but the v2 migration path was now
visible through successor-version headers and docs. The v1 removal
target is a future major version, no earlier than the documented sunset window.

## Releases

- **2026.5.0** shipped the first tagged baseline. It added the container path.
  It unified VM and container views. It improved PTY repair, live events, and
  raw logs.
- **2026.5.1** shipped the v2 runtime contract. It added admin routes,
  per-instance routes, AgentCard lookup, PTY binding, conformance checks, and
  the v1 migration path.
- **2026.5.2** kept the v2 release train moving. It added follow-up fixes and
  docs for the same surface.
- **2026.5.3** prepared the next release step. It removed legacy Python
  surfaces. It also moved release scripts forward.
- **2026.5.4** hardened safety and package handling. It also improved docs and
  team notes.
- **2026.5.5** continued the May baseline. It focused on runtime, CI, and docs.
- **2026.5.6** continued the same hardening path.
- **2026.5.7** moved package work forward.
- **2026.5.8** repaired release checks found by tag runs.
- **2026.5.9** kept the release gate work moving.
- **2026.5.10** improved VM E2E diagnostics.
- **2026.5.11** improved SSH wait behavior.
- **2026.5.12** fixed runner label access.
- **2026.5.13** repaired image checks and UEFI VM boot behavior.
- **2026.5.14** tightened release gates.
- **2026.5.15** improved image check diagnostics.
- **2026.5.16** refreshed user docs. Examples matched the live API, loadout
  registry, and image catalog.
- **2026.5.17** moved release jobs onto a proven runner lane. The prior tag was
  blocked by an upstream runner image pull.

## Packages & safety

May was a major safety month. The project pinned CI actions and container
images, pinned global npm installs, tightened cloud-init secret permissions,
redacted bearer tokens in WebSocket logs, removed the Docker socket from the
dev compose path, used constant-time secret checks, and moved HTTP and
WebSocket stacks toward rustls.

It also adopted AGPL licensing after the May release train. Downstream users
should review the license rules before sharing.

## Docs & builder experience

The docs gained a getting-started path, v2 migration guide, runtime reference,
contract specs, platform help matrix, glossary, concepts page, API sync, CLI
sync, WebSocket docs, release notes, and user runbook updates.

The CLI gained v2 admin migration commands, A2A task commands, AgentCard
checks, TUI driver commands, and more complete life cycle commands.

## Tests & CI

Testing moved from broad local checks toward release-gated proof. The repo added
conformance checks, OpenAPI coverage linting, VM E2E hardening, image smoke
checks, release attachment gates, schema linting, supply-chain pin checks, and
diagnostics for VM substrate failures.

The important change is behavioral: release flow should wait for the
runtime proof, not race ahead of it.

## Cross-project impact

The v2 runtime contract made Agentic Sandbox a stronger runtime for AIWG and
other agent workflows. AgentCard lookup, task routes, HITL prompts, and PTY
bindings give higher-level tools a documented way to use the sandbox.

The conformance harness also gives related projects a way to check whether a
runtime behaves as expected.

## Known issues & open threads

- Linux release and VM checks were the active proof points; macOS release
  assets still needed later work.
- VM startup and teardown remained host tied.
- Transport safety planning continued beyond May.
- Some early May releases were superseded by later tags after release-gate
  repairs.

## What's next

June would focus on secure transport, private VM agent enrollment, SSH gateway
access, live Observe/Drive uptime, credential proxy groundwork, and a more
complete public release surface.

## Links

- [Getting started](../getting-started.md)
- [v2 migration guide](../v2-migration-guide.md)
- [Runtime reference](../aiwg-executor.md)
- [Release checks](../releases/verification.md)
- [Security status](../security/security-status.md)

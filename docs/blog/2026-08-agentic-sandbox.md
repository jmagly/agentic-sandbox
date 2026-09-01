---
title: "agentic-sandbox — August 2026 Report"
date: "2026-08-31"
project: "agentic-sandbox"
type: report
tags: [report, "2026-08", "agentic-sandbox"]
summary: "August made agentic-sandbox easier to trust while it runs real work: clearer fleet state, better activity evidence, safer Docker control, VM package fixes, and the start of first-class Celld runtime integration."
hero: "https://docs.aiwg.io/agentic-sandbox/assets/blog/2026-08-agentic-sandbox.png"
hero_alt: "Sunlit glacier-blue glass forms rising from water, representing visible and bounded runtime state."
status: published
---

# agentic-sandbox — August 2026

*agentic-sandbox is a self-hosted place to run coding agents. It can run an agent in Docker, or in a small virtual machine, also called a VM. A VM gives each agent its own kernel. Docker starts faster and uses less. In both cases, the goal is the same: let an agent do real work without giving it the whole host.*

## TL;DR

August was about trust. The project made running sandboxes easier to check, recover, and explain. Fleet work now has saved records, so a restart does not erase what the system knew. Activity tools give you a better way to ask what happened during a run. Managed Docker also got a safer default path, with no control secrets placed in the normal container route.

Celld work moved forward too. August started the integration of Celld as a first-class runtime path. It is not the default runtime yet. The useful August story is that the runtime, test, storage, recovery, and operator groundwork is now in place so Celld can be fully tested and running later in the fall.

## By the Numbers

| What's public | Value |
|---|---|
| Runtime choices | Docker containers and KVM VMs |
| Release line | `v2026.7.20`, `v2026.8.3`, `v2026.8.5` |
| Install paths | GitHub installer, Linux packages, release archives |
| Key work | fleet state, activity evidence, safer Docker control, VM setup fixes, first-class Celld runtime integration |
| Source and docs | GitHub repo, GitHub Releases, docs.aiwg.io, aiwg.io sandbox page |

## Highlights

**Fleet state now survives restarts.** A fleet is a set of sandbox targets under one manager. This month, fleet work gained saved records for admission, inventory, replay, and restart checks. If the manager restarts, it can compare what it saved with what is still running.

**You get better answers to "what happened?"** Activity tools now track more metadata about process, file, network, and time-line events. That helps you compare what the agent said with what the host saw.

**Managed Docker has a safer default.** New managed Docker containers use a local socket path by default. The manager checks the local user identity on that path. It does not need to put a bearer token, client cert, or private key inside the default container route.

**Packaged VMs use the matching agent.** The August 1 corrective release fixed a package edge. VM setup now uses the `agent-client` shipped beside `agentic-mgmt`. It also waits until that agent is ready before it reports success.

**Celld moved toward a first-class runtime.** `v2026.8.5` added protected Celld runtime and qualification paths. The point is not that the runtime is finished. The point is that the integration path is now part of the product, with full testing and regular operation still expected later in the fall.

## Features Shipped

### Fleet State

Long agent jobs need boring state. They may run for hours. The manager may restart. The host may need to check what is still alive.

August added a steadier fleet model. A work request can be accepted, saved, observed, and checked again later. If the manager restarts, it can rebuild its view from saved records and live state.

Say you run a few agent jobs overnight. One writes code. One runs a check. One waits for a later task. In the morning, you want to know which work was accepted, which target ran it, and which work needs a retry. The fleet work makes that easier to see.

### Activity Evidence

Activity evidence helps answer a simple question: what did the sandbox see?

The August release added a stable event shape, loss-aware ingest, Linux metadata, macOS collection scaffolding, network metadata, time-line queries, governed export, and rolling evidence. The key word is evidence. If the system missed something, it can record loss or not enough proof. It should not turn a gap into a false pass.

For daily use, this helps after a failed task or a long run. You can search by time, source, runtime, and outcome. You can check whether the agent's story matches host-side facts. You can also see when the proof is not strong enough.

### Safer Docker Control

The managed Docker default changed in an important way. New managed containers use a host-mounted Unix-domain socket. That is a local path on the host. The operating system can tell the manager which local user is on the other end.

This lowers risk in the default route. A normal managed Docker path no longer needs a bootstrap bearer token, client cert, or private key inside that route. The control process and the workload also use different identities. Workload child processes drop Linux powers before running user code.

There are limits. Old managed containers are not changed in place. Recreate them to get the new boundary. Some explicit compatibility paths still exist. If a provider needs raw secrets, use the supported proxy path or choose a VM when you need stronger walls.

### VM Setup From Packages

The `v2026.7.20` release fixed VM setup from release packages. A public package includes `agentic-mgmt` and `agent-client`. VM setup now passes the matching `agent-client` into the VM and checks that it is the one that starts.

This matters when you install from a release. Before this fix, a VM could keep an older agent baked into its base image if a development build path was missing. The manager could look current while the agent inside the VM was old. Now a started VM must install the right agent and register before the start call succeeds.

### Celld as a First-Class Runtime Path

Celld is the durable-cell runtime path the project began integrating as a first-class runtime this month. August added protected runtime paths, fault checks, storage checks, recovery checks, and clearer operator notes.

The boundary still matters. Celld is not the default runtime yet, and full testing is still ahead. The useful change is that the project added the machinery needed to prove readiness, fail closed when proof is missing, and move toward regular operation later in the fall.

### AIWG And Cockpit

agentic-sandbox can be used on its own. It also sits under AIWG when isolation matters. Public AIWG docs describe the sandbox as the VM or Docker runtime that AIWG daemon and Cockpit can use.

Cockpit is the local operator console. agentic-sandbox is the executor it can front. Together, they separate the workflow layer from the runtime layer. AIWG helps decide and route work. agentic-sandbox keeps the work boxed in, visible, and easier to recover.

## Fixes

The VM package fix is the most visible one. Release installs now carry the matching agent client into VM setup. Started VMs must reach exact-agent readiness before success is reported.

Managed identity also improved. Docker Desktop behavior was corrected so the same managed identity model still holds across that boundary. Fleet bindings were hardened so work cannot be silently tied to a different runtime identity.

Celld integration work focused on state, callbacks, rollout plans, storage access, network checks, cleanup, and recovery. The public point is simple: unclear results now have to stay visible as proof to inspect, not as quiet success.

## Performance & Reliability

Reliability was the main theme. Saved fleet state makes restart recovery less fragile. Activity evidence gives operators better review tools. VM setup now waits for the real ready state. Build paths also gained bounds so package work is less likely to crowd out the host.

When a task is cut short, you want a record to inspect and a path to repair it. August moved more of that into released behavior.

## Breaking Changes & Migrations

Recreate existing managed Docker containers if you want the new default socket and split-identity boundary. Existing containers are not changed in place.

For VMs made with the earlier affected package, reprovision the VM or redeploy the matching `agent-client` before you rely on restart proof.

Celld is on the path toward full testing and regular operation later in the fall. Do not treat this month's work as a finished runtime promotion.

## Releases

- **`v2026.7.20`** (August 1, 2026) — fixed VM package setup so the packaged VM uses the matching `agent-client` and waits for readiness.
- **`v2026.8.3`** (August 4, 2026) — added saved fleet work, activity evidence, and the safer managed Docker default.
- **`v2026.8.5`** (August 23, 2026) — added protected Celld runtime and qualification work and hardened a libvirt checkpoint self-test.

## Dependencies & Security

Security work centered on identity and proof. The managed Docker path no longer puts control secrets into the default route. Workload code runs under a separate, low-power identity. Activity evidence adds governed export and checks for tampering.

The Apple developer package remains unsigned and for evaluation only. This report makes no Developer ID, notarization, stapling, or Gatekeeper claim.

## Docs & Developer Experience

The public README now gives a clearer setup path, package install flow, and VM versus Docker choice. It also links to release checks and the public setup manifest.

The release notes add operator guidance for managed Docker, VM package upgrades, Celld runtime qualification, and Apple package limits. The concepts docs explain the split between admin routes, per-agent task routes, and activity routes. That helps builders use the right surface for the right job.

## Tests & CI

Public release notes describe stronger checks around fleet restart behavior, activity proof, managed identity, VM readiness, and Celld test paths.

The test docs also keep the proof levels clear. A fast stub test can prove an API shape. A live runtime test proves more: task life cycle, terminal behavior, restart state, and policy limits.

## Cross-Project Impact

AIWG and Cockpit are the main public link. AIWG supplies the workflow and operator layer. agentic-sandbox supplies the isolated runtime underneath when work should run on hardware you control.

The August AIWG release notes also refer to Sandbox fleet and activity integration. That supports a public claim that the projects are aligning around safer local agent work.

## Known Issues & Open Threads

- Celld is still moving toward full testing and regular operation later in the fall.
- Some activity evidence is bounded test proof, not a longer production run.
- Native macOS activity collection still needs platform approval, signing, notarization, and host consent.
- The unsigned Apple package is for evaluation only.

## What's Next

Next work should keep pushing Celld from first-class integration into fully tested, regular runtime operation later in the fall. Fleet work should keep making restarts easier to recover from. Activity tools should keep showing what was seen, what was claimed, and what was missing.

For users, the direction is clear: safer default control, more honest evidence, and better recovery for long agent runs.

## Appendix

- **Public releases:** `v2026.7.20`, `v2026.8.3`, `v2026.8.5` on GitHub Releases.
- **Install paths:** GitHub installer, Linux packages, release archives, checksums, and source archives.
- **Public source:** `https://github.com/jmagly/agentic-sandbox`
- **Public release notes:** `https://github.com/jmagly/agentic-sandbox/releases`
- **Public docs:** `https://docs.aiwg.io/` and `https://aiwg.io/sandbox/`
- **Window:** August 2026, researched through August 28, 2026.

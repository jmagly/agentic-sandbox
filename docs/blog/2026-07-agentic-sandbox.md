# July 2026: Faster starts and clearer sandbox choices

**Published:** 2026-07-31  
**Project:** Agentic Sandbox  
**Window:** July 2026  
**Release line covered:** `v2026.7.x`

Agentic Sandbox gives AI agents a controlled place to work. They can run code, keep sessions alive, and use a host, a container, or a virtual machine. Each path limits what the agent can reach.

July made that choice easier. Starts got faster. The control API became clearer. Mac support moved forward. Releases gained more signing and checks.

## TL;DR

- Cloud Hypervisor work opened a path to faster VM start and restore.
- Launch options are clearer, so a caller can ask for the style it needs.
- The control API shows more readiness and fit data.
- Mac host and Docker work moved forward, with Apple Silicon in mind.
- The identity and key boundary is clearer.
- Release signing and publish checks got stronger.

## By the numbers

| Public surface | Current state |
|---|---|
| Runtime choices | Host, container, and VM paths |
| July release line | `v2026.7.x` |
| Key capabilities | launch options, Cloud Hypervisor work, control discovery, Mac package work, identity boundary |
| Docs | `https://docs.aiwg.io/agentic-sandbox/` |
| Source | `https://github.com/jmagly/agentic-sandbox` |

## Highlights

### Faster VM work

Cloud Hypervisor work touched checkpoint, restore, warm pools, fork, network, and GPU paths. The goal is simple. Keep the stronger VM wall. Cut the wait.

Use case: you want an agent in a stronger sandbox. You do not want every run to feel like a cold boot. A prepared VM path can help.

### Launch options are clearer

The API now has a clearer way to ask for a launch style. A caller does not need to imply its needs through scattered settings.

That helps dashboards, command-line tools, and scripts. They can ask for the right path. They can also get a clear answer when the host cannot provide it.

### Control discovery is easier

The control side now shows more readiness data. It also shows which providers and launch paths fit the host.

This gives users a safer way to inspect the sandbox before starting work. It also helps Cockpit avoid hidden guesses.

### Mac support moved forward

July added Mac host and Docker groundwork, with Apple Silicon in mind. This does not mean every feature is equal on every platform. It means the local Mac path is getting clearer.

Many developers run agents from laptops. A local path is easier to try when install and key storage fit the host.

### The key boundary is clearer

The key work defines how one side proves who it is. That matters because sandbox control is a trust boundary, not just an API.

## Features shipped

### Run paths and fast starts

July added checkpoint and restore work, Cloud Hypervisor support, warm pools, fork-from-base work, and checks around observe and cleanup. These pieces make the VM path more useful for repeat runs.

The value is not only speed. A fast VM that loses state is not useful. A fast VM that weakens the wall is worse. This work pairs speed with cleanup and secret care.

### API and control shape

The API now shows more about what the host can run, which provider paths are ready, and which tool sets fit. A tool set is the set of commands and setup an agent gets at start.

This helps callers choose a path without hard-coding host details. If a VM path is missing, the caller can see that. If a launch style needs a prepared file, the control layer can show it.

### Mac and Apple Silicon

The Mac line gained host and Docker work, network handling, package checks, Keychain key storage, and Apple Silicon image work. The aim is a local package path that can be checked cleanly.

For users, the practical win is fewer special steps when testing sandboxed agent work on a Mac.

### User controls and structured output

The UI work continued around structured output, session detail, reconnect controls, and reprovisioning. Structured output means an agent can send more than raw terminal text. A UI can show status, chat-like messages, and task output in a clearer way.

### Release checks

The release path got stronger around signed files, packages, SBOM output, source packages, and registry mirrors. An SBOM is a list of what is inside a release. It makes the release easier to inspect later.

## Fixes

July fixes focused on run health, CI, Mac packages, containers, and releases. Non-root container provider work was unblocked. Managed containers now start with egress denied unless it is approved. Session and control-channel recovery got stronger.

Several CI fixes covered VM storage, image builds, Mac checks, package signing, and runner behavior. These are not flashy features. They keep the public release path believable.

## Performance & reliability

The speed story is the VM fast-start path. Checkpoint, restore, warm pool, and Cloud Hypervisor work all point toward faster starts for stronger sandbox sessions. Reliability work made session recovery, agent registration, container readiness, release signing, and Mac package checks steadier.

## Breaking changes and migrations

No broad user migration is called out for normal installs. If you maintain a custom launch path, review the launch-options shape before adopting the July line.

## Releases

July `v2026.7.x` releases covered launch options, Cloud Hypervisor paths, structured output, Mac groundwork, identity contracts, release signing, and publish fixes. Some tags were corrective cuts where the run code stayed the same while the publish path was fixed.

## Dependencies and security

Security work focused on identity boundaries, snapshot secret handling, safer container defaults, signed releases, and SBOM output. The project also tightened release key handling and registry publish checks.

## Docs and developer experience

Docs now explain more of the control API, launch options, monitoring, troubleshooting, Mac checks, and trust boundary. That helps users understand the sandbox before they build around it.

## Tests and CI

Tests and CI expanded around Mac smoke checks, Cloud Hypervisor behavior, VM restore paths, package builds, release signing, images, and control contracts. The important result is trust: the same boundaries in docs are checked in release work.

## Cross-project impact

AIWG Cockpit depends on these run contracts to start and watch sessions cleanly. AIWG release and skill work benefits from a stronger sandbox. Fortemi and Pagenary can use the same controlled run layer when they need agent work with a clear wall.

## Known threads

Cloud Hypervisor and GPU work are active run paths. They are not a blanket promise that every host works the same way. Mac support is moving forward, but users should follow the documented checks for their host. Enterprise key-service details stay outside this public report.

## What is next

Expect more work on run choice, VM fast starts, Mac packages, control discovery, identity behavior, and release checks.

## Links

- [Getting started](../getting-started.md)
- [Release verification](../releases/verification.md)
- [Security status](../security/security-status.md)

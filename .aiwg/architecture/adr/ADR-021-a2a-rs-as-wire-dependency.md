# ADR-021: `a2a-rs` as Wire-Format Dependency via Gitea Mirror

## Status

Accepted (2026-05-09)

## Context

ADR-018 commits to A2A as the base protocol. The reference implementation requires Rust types matching A2A's data model, transport bindings (HTTP+JSON/REST first, JSON-RPC and gRPC follow), and the AgentCard/Task/Message/etc. wire format.

The official Rust SDK `a2aproject/a2a-rs` exists:
- 72 releases on crates.io as of late April 2026.
- Workspace: `a2a` (core types), `a2a-client` (REST + JSON-RPC + gRPC + SLIMRPC clients), `a2a-server` (axum-based server framework), `a2a-grpc`, `a2a-pb` (protobuf), `a2a-slimrpc`, `a2acli`.
- Active development; some open issues. Doesn't claim full A2A v1 spec coverage — claims wire-compatibility with other A2A SDKs.

We forked it as `jmagly/a2a-rs` per our standard pattern: fork the upstream, mirror to Gitea, depend on the mirror, control update cadence. Same pattern we applied to the A2A spec (`jmagly/A2A` → `roctinam/A2A`).

The alternative — writing our own `a2a-types` from scratch from `a2a.proto` — is technically possible but:
- Reinvents work that's already published, tested, and aligned with the spec.
- Misses upstream improvements (bug fixes, schema additions, performance).
- Makes our crate harder to upstream-contribute to (different types means our work doesn't translate back).

## Decision

**`agentic-sandbox-executor` crate depends on `a2a-rs` via the Gitea mirror of the `jmagly/a2a-rs` fork.**

### Mirror setup

- Upstream: `github.com/a2aproject/a2a-rs` (canonical, governed by A2A TSC + a2a-rs maintainers)
- Our fork: `github.com/jmagly/a2a-rs`
- Our mirror (gate-on-update): `git.integrolabs.net/roctinam/a2a-rs`
- Build dependencies pull from the Gitea mirror (HTTPS or SSH). Public read access on Gitea side.

### Update workflow

Per our standard pattern (saved in feedback memory):

1. Upstream `a2aproject/a2a-rs` releases new version.
2. Update `jmagly/a2a-rs` fork: `git fetch upstream && git merge upstream/main` (or rebase).
3. Push to Gitea mirror: `git push gitea --mirror`.
4. Update `agentic-sandbox-executor` Cargo manifest with the new commit/tag pin.
5. CI runs against the new version; conformance harness verifies no regressions.
6. If approved, merge the bump.

This gives us:
- **Update gate**: nothing hits our build until we explicitly bump.
- **Upstream-divergence option**: if upstream takes a direction we can't follow, we maintain our fork locally (vendor patches in our fork repo).
- **Contribution path**: our improvements can flow upstream via PR from `jmagly/a2a-rs` to `a2aproject/a2a-rs`.

### Cargo dependency form

```toml
[dependencies]
a2a = { git = "https://git.integrolabs.net/roctinam/a2a-rs.git", tag = "agentic-sandbox-v2.0.0", package = "a2a" }
a2a-server = { git = "https://git.integrolabs.net/roctinam/a2a-rs.git", tag = "agentic-sandbox-v2.0.0", package = "a2a-server" }
a2a-client = { git = "https://git.integrolabs.net/roctinam/a2a-rs.git", tag = "agentic-sandbox-v2.0.0", package = "a2a-client" }
```

We pin to **tags we create** in the fork (e.g. `agentic-sandbox-v2.0.0`) rather than upstream tags directly, so our pin is stable even if upstream retags or rebases.

### Workspace inclusion (alternative considered)

Including the entire a2a-rs workspace as a `cargo workspace` member of `agentic-sandbox` was considered. Rejected: tightly couples our build, makes upstream merges painful. Git dependency on the mirror is the right boundary.

### Coverage gaps

`a2a-rs` doesn't claim full A2A v1 coverage. We will:

1. Audit which A2A operations and types are present in `a2a-rs` vs. spec.
2. File issues upstream (in `jmagly/a2a-rs` first, then upstream PR if appropriate) for missing pieces we need.
3. Implement missing pieces in our fork while upstream PR is in flight.
4. Once upstream lands, drop our patch.

Our extensions (`runtime/v1`, `hitl-prompt/v1`, etc.) and our PTY custom binding are NOT contributed upstream as part of `a2a-rs` (they're agentic-sandbox-specific). They live in our crate; types reference `a2a-rs` core types.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. Depend on `a2a-rs` via Gitea fork mirror (chosen)** | Aligned with project's fork-as-update-gate pattern; reuses upstream work; contribution-path open | Maintenance: keeping fork synced |
| B. Vendor `a2a-rs` source into our repo | Maximum isolation | Lose upstream updates entirely; merge nightmare |
| C. Direct dep on upstream crates.io | Simplest | No update gate; CVE in upstream auto-pulls into our build |
| D. Write our own `a2a-types` from `a2a.proto` | Maximum control | Reinvents tested code; diverges from ecosystem; harder upstream contribution |

## Consequences

### Positive

- Aligned with our standard fork-and-mirror pattern (consistent with how we handle the A2A spec itself, AIWG, etc.).
- Free upstream work: 72 releases of bug fixes, schema correctness, transport bindings.
- Clear contribution path: our improvements can flow upstream.
- Update cadence is ours: nothing arrives in our build without explicit bump.
- Crate pinning by tag we control: stable even across upstream retag.

### Negative

- Maintenance overhead: someone has to keep the fork synced with upstream. Recommend: monthly cadence at minimum; on-demand if upstream ships a CVE fix.
- If `a2a-rs` upstream stalls or diverges from a direction we need, we maintain our fork independently (acceptable risk; the same applies to any fork-mirror dep).
- Our extension/binding code in `agentic-sandbox-executor` may need types not yet in `a2a-rs`; we either contribute upstream first or ship a parallel definition.

### Neutral

- `a2a-rs` is Apache 2.0 licensed; same as A2A spec; no license friction.

## Implementation Notes

- Repo creation on Gitea blocked on MCP token scope (`write:user`); operator creates `roctinam/a2a-rs` empty; then `git push --mirror` from local clone.
- Initial fork sync done at `jmagly/a2a-rs`; we baseline from there.
- CI hook: nightly check for `jmagly/a2a-rs` updates; alert on new commits.
- Tagging convention: `agentic-sandbox-v<X>.<Y>.<Z>` for our pin tags (independent of upstream version numbers).
- Our `agentic-sandbox-executor` crate Cargo.toml is the canonical place where the version pin lives.

## Related

- ADR-018 (A2A as base protocol — drives the need for a2a-rs)
- ADR-019 (extensions; our crate adds extensions on top of a2a-rs types)
- ADR-020 (PTY custom binding; implements on top of a2a-rs server framework)
- Project pattern: fork-as-update-gate (also applied to A2A spec, AIWG)
- Memory: feedback_single_dev_workflow.md (commit-to-main pattern)

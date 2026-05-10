# Upstream Sync — A2A spec and Rust SDK

This project depends on two upstream repositories that we mirror into Gitea as a fork-as-update-gate:

| Upstream (GitHub) | Mirror (Gitea) | Role |
|---|---|---|
| `jmagly/A2A` | `roctinam/A2A` | A2A protocol specification fork |
| `jmagly/a2a-rs` | `roctinam/a2a-rs` | Rust SDK; Cargo wire dependency (ADR-021) |

Nothing from upstream hits our build until we deliberately bump. Our Cargo manifests pin against a Gitea-hosted tag on `roctinam/a2a-rs`.

## Cadence

- **Monthly**: review upstream commits and decide whether to bump.
- **On-demand**: any upstream CVE fix or correctness fix touching our usage path is pulled within one business day.

## Sync procedure

Both clones live under `/home/roctinam/dev/`. Each has two remotes:

- `origin` — the upstream GitHub repo (read-only for us)
- `gitea` — the Gitea mirror (we push)

### Pull upstream changes

```bash
cd /home/roctinam/dev/A2A
git fetch origin
git push gitea --mirror

cd /home/roctinam/dev/a2a-rs
git fetch origin
git push gitea --mirror
```

### Bump our Cargo pin

After pulling new commits into `roctinam/a2a-rs`, decide whether to advance the Cargo pin:

1. Review changes against our usage in `agentic-sandbox-executor`.
2. If safe, tag a new baseline:

   ```bash
   cd /home/roctinam/dev/a2a-rs
   git tag -a agentic-sandbox-v<NEW_VERSION> -m "Cargo pin baseline for agentic-sandbox v<NEW_VERSION>"
   git push gitea agentic-sandbox-v<NEW_VERSION>
   ```

3. Update `Cargo.toml` in agentic-sandbox to point at the new tag and run the conformance harness.

The current baseline is `agentic-sandbox-v2.0.0` (created 2026-05-10).

## Upstream-check automation

A nightly job watches `jmagly/a2a-rs` for commits not present in `roctinam/a2a-rs` and posts an alert to the team channel. The job is intentionally read-only — it never auto-pushes. Sync is always a deliberate operator action.

Workflow location: TBD (tracked separately; see #197 follow-up).

## Why fork-as-update-gate?

We did not vendor the upstream code into our repo because:

1. Vendoring buries provenance and obscures version drift.
2. Pulling directly from GitHub exposes our build to upstream incidents (force-push, repo deletion, supply-chain compromise).
3. A Gitea mirror with operator-controlled tags gives us reproducibility without losing the ability to take fixes quickly.

See ADR-021 for the original decision and feedback memory `feedback_single_dev_workflow.md` for the project-wide pattern.

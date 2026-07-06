# Doc Sync Audit - 2026-07-06 Release

- Trigger: release gate `doc-sync-code-to-docs`
- Direction: code-to-docs
- Scope: release-v2026.7.2
- Since: v2026.7.1
- Auditors: release-delta, session-protocol, cli-dashboard, docs-release-surface

## Summary

The changed code surface for v2026.7.2 is concentrated in the formal session
registry, agent-scoped session list APIs, WebSocket session listings,
`sandboxctl session` display/attach behavior, and dashboard existing-session
cards. User-facing docs already covered the new REST list shape, formal
controller lease semantics, replay fallback, CLI reconnect expectations, and
dashboard reconnect metadata. Release-facing docs were missing and have been
added.

## Findings

| Severity | Finding | Resolution |
| --- | --- | --- |
| High | Release notes did not describe the controller-lease behavior change. | Added `CHANGELOG.md` entry and `docs/releases/v2026.7.2.md`. |
| High | Release notes did not call out the richer session-list attach metadata needed by reconnecting clients. | Added highlights, upgrade matrix, and verification steps for API, CLI, dashboard, and operators. |
| Medium | Doc-sync bookkeeping still pointed at the v2026.7.1 release audit. | Updated `.aiwg/.last-doc-sync` and `.aiwg/reports/doc-sync-last-run.json`. |
| Low | Existing API/WebSocket/TUI docs needed confirmation against the implementation delta. | Confirmed docs include idempotent create, agent-scoped session list metadata, singleton controller lease, and no-keyframe replay fallback. |

## Files Modified

- `CHANGELOG.md`
- `docs/releases/v2026.7.2.md`
- `docs/releases/_manifest.json`
- `.aiwg/.last-doc-sync`
- `.aiwg/reports/doc-sync-last-run.json`
- `.aiwg/reports/doc-sync-audit-2026-07-06-release.md`

## Validation

- `git diff --check`
- `node --check management/ui/app.js`
- `cargo test --manifest-path management/Cargo.toml --lib`
- `cargo test --manifest-path agent-rs/Cargo.toml --lib`
- `cargo test --manifest-path cli/Cargo.toml --bins`

## Remaining Manual Review

- Tag publication gates still require a clean worktree, a release/version bump
  commit, green CI on `main`, and the configured tag publication flow.

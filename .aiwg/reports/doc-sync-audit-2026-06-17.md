# Doc Sync Audit - 2026-06-17

Direction: code-to-docs

Scope:
- Release range `v2026.6.15..HEAD`
- GitHub release checksum mirror behavior
- v2 admin host-runtime instance listing behavior
- CalVer release documentation for `2026.6.16`

## Summary

Code-to-docs sync found two release documentation updates required before
tagging `v2026.6.16`:

- `CHANGELOG.md` did not yet describe the GitHub aggregate checksum mirror fix.
- `CHANGELOG.md`, `docs/releases/`, and the release manifest did not yet
  describe the host-runtime admin listing fix.

## Changes Applied

- Bumped Cargo manifests and lockfiles from `2026.6.15` to `2026.6.16` with
  `scripts/bump-version.sh`.
- Populated the `CHANGELOG.md` `2026.6.16` section from the release commit
  range.
- Added `docs/releases/v2026.6.16.md`.
- Added `v2026.6.16` to `docs/releases/_manifest.json`.

## Validation

- `git diff --check` passed.
- `cargo test --manifest-path management/Cargo.toml admin_v2::tests::registered_host_context_maps_to_admin_instance` passed.
- `cargo pkgid --manifest-path management/Cargo.toml` reported `agentic-management@2026.6.16`.
- `cargo pkgid --manifest-path agent-rs/Cargo.toml` reported `agent-client@2026.6.16`.
- `cargo pkgid --manifest-path cli/Cargo.toml` reported `agentic-cli@2026.6.16`.
- `python3 scripts/check-doc-links.py --docs-root docs` passed.

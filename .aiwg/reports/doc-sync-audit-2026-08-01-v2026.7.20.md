# Doc Sync Audit Report

- Reviewer: Technical Writer
- Date: 2026-08-01
- Release: v2026.7.20
- Scope: code-to-docs dry-run audit only
- Status: PASS

## Summary

I reviewed the requested code, script, release-note, manifest, and version metadata files for v2026.7.20. The audited code paths and documentation are aligned on the two release-critical changes: the exact packaged `agent-client` handoff through `AGENT_CLIENT_SOURCE_BIN`, and the `--wait-ready` requirement for started VM provisioning. The release manifests and Cargo version files also reflect v2026.7.20.

## Issues Found

### Minor

- Resolved: `docs/releases/v2026.7.20.md` now links its final “Full notes”
  reference directly to the v2026.7.20 changelog section.

## Clarity Improvements

- The release note already states the behavioral change clearly. No additional clarity fixes were required in the audited scope.

## Consistency Fixes

- Manifest ordering is consistent: `docs/releases/v2026.7.20.md` is listed first in `docs/releases/_manifest.json`, and `docs/_manifest.json` includes the new release page.
- Version numbers are consistent across `management/Cargo.toml`, `agent-rs/Cargo.toml`, and `cli/Cargo.toml`.

## Structure Enhancements

- None required for the audited files.

## Sign-Off

- Status: PASS
- Conditions: none.
- Rationale: no code-to-doc mismatch remains in the requested scope.

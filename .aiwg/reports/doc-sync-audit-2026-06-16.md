# Doc Sync Audit - 2026-06-16

Direction: code-to-docs

Scope:
- gRPC agent transport CA backend lifecycle
- Release flow configuration
- Top-level roadmap and changelog release notes

## Summary

Code-to-docs sync found two release-blocking documentation drifts:

- README roadmap still described authenticated agent transports as pending.
- CHANGELOG `Unreleased` did not mention the CA backend lifecycle or the new
  direct-delivery CalVer release flow config.

## Changes Applied

- Updated `README.md` to mark authenticated agent transports complete and point
  operators to `docs/security/agent-transport-ca-backends.md`.
- Added `CHANGELOG.md` `Unreleased` entries for the gRPC CA backend lifecycle
  and `.aiwg/release.config`.

## Validation

- Existing CA backend operations doc is present in `docs/_manifest.json`.
- Release config validates against the AIWG release-config schema.
- CI and npm pin lint checks pass.

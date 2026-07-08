# Monthly Report - 2026-04

Backfilled: 2026-07-08  
Scope: April 2026

## Summary

April continued VM and management-surface development, especially loadout
manifests, provider profiles, HITL endpoints, CLI command structure, and
session cleanup tests. Like March, April had implementation artifacts but no
formal monthly report or release note checked in.

## Evidence Reviewed

- `images/qemu/loadouts/layers/base-dev.yaml`,
  `images/qemu/loadouts/layers/base-minimal.yaml`,
  `images/qemu/loadouts/layers/docker.yaml`,
  `images/qemu/loadouts/layers/network-tools.yaml`, and
  `images/qemu/loadouts/layers/observability.yaml` show loadout layer work.
- `images/qemu/loadouts/providers/claude-code.yaml` and
  `images/qemu/loadouts/profiles/*.yaml` show provider/profile expansion.
- `images/qemu/loadouts/resolve-manifest.sh` records manifest resolution.
- `management/src/http/loadouts.rs` and
  `management/src/http/loadout_registry.rs` expose loadout data to the
  management API.
- `management/src/http/hitl.rs` and CLI command files under `cli/src/cmd/`
  show HITL and CLI surface work.
- `agent-rs/tests/session_cleanup_test.rs` records agent session cleanup test
  coverage.
- `.aiwg/architecture/SESSION_PROTOCOL_CONTRACTS.md` captures protocol design.

## Delivered

- Loadout layer/profile model for QEMU VM provisioning.
- Management API surfaces for loadout discovery.
- CLI command expansion across attach, exec, health, HITL, and loadout paths.
- Session cleanup regression tests for the Rust agent.
- Session protocol contract documentation.

## Gaps And Carryover

- No April monthly report, release announcement, or doc-sync audit existed
  before this backfill.
- Provider install parity and session durability continued to require later
  hardening.

## Verification Snapshot

No April-specific test transcript was found. This report records checked-in
artifact evidence and known carryover only.


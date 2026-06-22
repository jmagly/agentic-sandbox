# Doc Sync Audit - 2026-06-22

## Summary

- Direction: `code-to-docs`
- Scope: incremental release audit for gateway SSH hardening
- Trigger: release prep after `41f7afe` (`security(gateway): harden SSH access paths`)
- Result: PASS

The changed code surface is the gateway SSH connector, SSH certificate lease
API, operator identity binding, and SSH lease response semantics. Public API
documentation was reconciled in the implementation commit before this report.

## Code Surface Audited

- `management/src/ssh_gateway_connector.rs`
- `management/src/http/ssh_gateway.rs`
- `management/src/http/operator_auth.rs`
- `management/src/ssh_gateway.rs`
- `management/src/main.rs`

## Documentation Surface Audited

- `docs/API.md`
- `CHANGELOG.md`
- `docs/releases/`
- `.aiwg/planning/ssh-gateway-access-rollout-2026-06-19.md`
- `.aiwg/architecture/adr/ADR-029-gateway-terminal-access-options.md`
- `.aiwg/architecture/reviews/2026-06-20-ssh-key-rotation-disposition.md`

## Findings

### DOC-DRIFT-001 - SSH connector routing policy

Status: fixed before report.

The connector now requires an explicit `AGENTIC_GATEWAY_SSH_ALLOWLIST` when
`AGENTIC_GATEWAY_SSH_LISTEN` is enabled. `docs/API.md` documents the
`actor=instance` allowlist format, wildcard behavior, and the relationship
between CLI actor selection and connector authorization.

### DOC-DRIFT-002 - SSH lease API actor binding

Status: fixed before report.

Lease issue/list/get/revoke handlers now require an authenticated operator
identity, and lease issuance derives actor metadata from that identity instead
of trusting request-body `actor`. `docs/API.md` states that the lease API
requires authenticated operator identity and that lease actor metadata comes
from the authenticated caller.

### DOC-DRIFT-003 - Revocation semantics

Status: fixed before report.

SSH lease responses now include
`revocation_effect: "metadata_only_until_certificate_expiry"`. `docs/API.md`
documents that revocation marks gateway lease metadata as revoked while
already-returned OpenSSH certificates remain governed by their short validity
window until runtime-enforced revocation is added.

### DOC-DRIFT-004 - Prelude size bound

Status: no public doc change required.

The connector now enforces `MAX_PRELUDE_BYTES` while reading, before a newline
is required. This is an implementation hardening detail; the public connector
prelude format in `docs/API.md` remains accurate.

### DOC-DRIFT-005 - Dated planning artifacts still use future tense

Status: accepted as historical context.

The dated rollout and disposition artifacts under `.aiwg/` still describe the
SSH gateway lease backend as planned or future work. They are retained as
historical planning/review records rather than rewritten in place. The current
user-facing source of truth is `docs/API.md`, and the release notes for the next
tag should describe the shipped hardening.

## Validation

- Gitea Actions run 1613 on `main` / `41f7afe`: success
  - Lint: success
  - Test: success
  - Build: success
  - Docker Build & Publish: success
  - E2E Tests: success
  - Security Scan: success
- Gitea Actions run 1614 on `main` / `41f7afe`: conformance success
- Local pre-push verification:
  - `make check`
  - `cd management && cargo check --workspace`
  - `make lint`
  - `git diff --check`

## Manual Review

No unresolved user-facing documentation drift was found for the changed gateway
SSH release surface.

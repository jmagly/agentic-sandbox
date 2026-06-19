# Doc Sync Audit - 2026-06-19

Direction: code-to-docs

Scope:
- `management/src/main.rs`
- `management/src/grpc.rs`
- `agent-rs/src/main.rs`
- `docs/`
- `README.md`
- `management/README.md`
- `agent-rs/README.md`
- release artifacts for `2026.6.25`

## Summary

The release-prep sync reconciled the bootstrap-enrolled mTLS path from issue
#514 with operator-facing documentation.

## Findings

| ID | Severity | Finding | Resolution |
| --- | --- | --- | --- |
| DOC-DRIFT-001 | High | Static-cert gRPC mTLS listener behavior was covered by tests but not described in transport CA/backend docs. | Updated `docs/security/agent-transport-ca-backends.md` with the SPIFFE URI-SAN peer identity contract and `x-agent-instance-id` match requirement. |
| DOC-DRIFT-002 | Medium | gRPC architecture docs described client certificate validation but did not state the runtime identity comparison used after TLS. | Updated `docs/grpc-architecture.md` to document SPIFFE `/agent/<instance_id>` matching. |
| DOC-DRIFT-003 | Medium | Getting-started and management docs did not clearly separate the one-time HTTP bootstrap endpoint from the long-lived gRPC mTLS endpoint. | Updated `docs/getting-started.md` and `management/README.md` with Docker-reachable HTTP bootstrap guidance. |
| DOC-DRIFT-004 | Low | Agent troubleshooting docs did not mention that connection failures now include the full underlying error chain. | Updated `agent-rs/README.md`. |
| DOC-DRIFT-005 | Low | Release notes and changelog did not cover the #514 verification and launch security posture docs. | Added `CHANGELOG.md` entry and `docs/releases/v2026.6.25.md`. |

## Validation

- Version references use CalVer `2026.6.25` with no leading zeros.
- Internal paths referenced by this report exist in the worktree.
- Release artifacts include changelog and `docs/releases/v2026.6.25.md`.

## Residual Notes

The existing dev server smoke failed before mTLS when its HTTP listener was
loopback-only and containers attempted `host.docker.internal:8122`. An isolated
high-port smoke with Docker-reachable HTTP bootstrap and static gRPC mTLS
proved enrollment, mTLS client auth, peer identity extraction, registration,
and continued metrics.

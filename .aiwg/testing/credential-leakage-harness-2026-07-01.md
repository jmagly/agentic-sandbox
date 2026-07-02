# Credential leakage harness evidence

Date: 2026-07-01T20:32:51-04:00

## Scope

This harness covers deterministic credential non-exposure checks for:

- HTTP credential proxy allowed, denied, rate-limited, wrong-scope, revoked, expired, and policyless paths.
- Credential metadata and lease API responses.
- Startup profile metadata and inline secret rejection.
- PTY transcript hot replay and archive redaction.
- QEMU loadout generated cloud-init credential reference policy.

Direct upstream bypass is not claimed as denied by this harness. Profiles
without an egress allowlist are reported as unsupported for broad proxy
non-exposure claims; use network-policy or allowlist verification before
claiming bypass prevention.

## Results

| # | Status | Command | Evidence |
| --- | --- | --- | --- |
| 1 | pass | `cargo test --manifest-path management/Cargo.toml credential_proxy -- --nocapture` | Completed; no sentinel appeared in harness output. |
| 2 | pass | `cargo test --manifest-path management/Cargo.toml credentials -- --nocapture` | Completed; no sentinel appeared in harness output. |
| 3 | pass | `cargo test --manifest-path management/Cargo.toml startup_profile -- --nocapture` | Completed; no sentinel appeared in harness output. |
| 4 | pass | `cargo test --manifest-path management/Cargo.toml transcript_query_redacts_provider_secrets_from_hot_and_archive_records -- --nocapture` | Completed; no sentinel appeared in harness output. |
| 5 | pass | `bash images/qemu/loadouts/tests/test_generate_from_manifest.sh` | Completed; no sentinel appeared in harness output. |

## Sentinel Scan

No configured sentinel value appeared in captured harness command output.

Sentinels scanned:
- `proxy-secret-fake`
- `sk-not-real`
- `sk-ant-not-real`
- `sk-test`
- `sk-test-readiness-secret`

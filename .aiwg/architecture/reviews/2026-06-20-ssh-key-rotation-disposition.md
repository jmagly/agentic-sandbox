# SSH Key Rotation Disposition

Date: 2026-06-20

Issues: #537, #531, #533, #535

## Summary

`management/src/audit/secrets_rotation.rs` contains persistent SSH keypair
rotation for VM-oriented `ssh_key` secrets. That model predates ADR-029 and is
not the planned managed-profile SSH access model.

Disposition: retain only as a legacy direct-runtime dev/break-glass helper
until #531 introduces gateway-mediated SSH certificate or lease issuance. Do
not reuse its persistent private-key storage semantics for gateway SSH access.

## Current Implementation

| Path | Behavior | Disposition |
| --- | --- | --- |
| `RotationConfig::ssh_key_rotation_days` | Schedules rotation by days. | Legacy direct-runtime key setting only. |
| `RotationConfig::ssh_keys_dir` | Stores generated key versions on disk. | Not acceptable for gateway SSH cert lease state. |
| `SecretsRotator::rotate_ssh_keys` | Generates an Ed25519 keypair with `ssh-keygen`, stores versioned private/public key files, hashes the public key, and cleans older versions. | Retain only for dev/break-glass direct runtime SSH; refactor/remove when gateway lease backend lands. |
| `SecretsRotator::revoke_secret` for `ssh_key` | Deletes stored key versions. | Useful cleanup behavior for retained direct-runtime path. |
| `management/src/audit/mod.rs` rustdoc | Advertised generic SSH key rotation. | Updated to call this legacy direct-runtime SSH key rotation and separate ADR-029/#531 gateway SSH cert leases. |

## Gateway SSH Lease Requirements

Future gateway SSH work must use a separate model:

- short-lived SSH certificates or session-scoped ephemeral keys;
- actor, instance id, principal, access mode, and TTL bound into lease/audit
  metadata;
- no durable private key material in session records, logs, replay metadata, or
  operation results;
- audit event names that distinguish provider SSH credentials, direct-runtime
  dev/break-glass keys, and gateway SSH certificate leases;
- #533 leakage tests covering whichever credential path remains.

## Credential Path Naming

Use distinct names in code, audit, docs, and tests:

| Credential path | Current or planned owner | Naming requirement |
| --- | --- | --- |
| Provider/workload SSH credentials | Image entrypoints and automation credential loaders such as `images/common/automation-control/ssh-automation.sh` and provider-specific agent entrypoints. | Name as provider or workload credentials. Do not describe them as runtime access keys. |
| Direct-runtime dev/break-glass SSH keys | QEMU provisioning/dev scripts and `management/src/audit/secrets_rotation.rs` `ssh_key` metadata. | Name as legacy direct-runtime or dev/break-glass SSH keys. Do not describe them as managed-profile defaults. |
| Gateway SSH access certificates or leases | Future #531 backend. | Name as gateway SSH certificate leases and audit lease id, actor, instance, principal, TTL, and outcome without secret material. |

Because the retained `ssh_key` rotator still stores durable private key files,
#533 must test for leakage of the remaining direct-runtime dev/break-glass key
path as long as it exists. When #531 replaces this path with gateway SSH
certificate leases, #533 should assert that private key and certificate material
does not appear in logs, env, session records, operation results, or PTY replay
metadata.

## Documentation Changes

- `management/src/audit/secrets_rotation.rs` now states that `ssh_key` is not
  the ADR-029 gateway certificate lease backend.
- `management/src/audit/mod.rs` no longer advertises generic SSH key rotation.
- `docs/DEPLOYMENT.md` and
  `docs/reliability-implementation-checklist.md` now refer to transport
  credentials and legacy direct-runtime SSH keys rather than generic agent
  secrets/default SSH key rotation.

## Verification

- `cargo fmt --all --check` from `management/`
- `cargo test test_rotator_creation --lib` from `management/`
- `rg -n "SSH keys|SSH key rotation|agent secrets|Secret rotation" management/src/audit docs/DEPLOYMENT.md docs/reliability-implementation-checklist.md`

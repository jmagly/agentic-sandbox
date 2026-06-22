# AIWG Direct SSH Proxy Disposition

Date: 2026-06-22

Issues: #540, #531, #534, #535, #537

## Summary

`management/src/http/aiwg_proxy.rs` contains legacy AIWG companion endpoints
for manifest CRUD and allowlisted `aiwg exec`. The implementation shells out to
host `ssh` and reaches the runtime directly with the same runtime key material
used by older deployment helpers.

Disposition: retain only as an explicitly enabled dev/break-glass diagnostic
bridge while #531 designs the gateway-mediated SSH certificate lease backend.
Do not present this path as managed-profile SSH access, and do not reuse its
long-lived runtime key semantics for gateway SSH.

## Current Implementation

| Path | Behavior | Disposition |
| --- | --- | --- |
| `ssh_key_path` | Resolves `~/.config/agentic-sandbox/secrets/ssh-keys/{vm}` or `~/.ssh/agentic_ed25519`. | Legacy direct-runtime key lookup only. |
| `ssh_exec` | Runs a remote shell command through `ssh agent@{ip}`. | Retain only behind dev/break-glass gate until #531 replacement. |
| `ssh_write_file` | Writes manifest content through `ssh` stdin redirection. | Retain only behind dev/break-glass gate until #531 replacement. |
| `list_manifests`, `get_manifest`, `push_manifest` | Manifest CRUD over the legacy SSH bridge. | Disabled by default; requires `AGENTIC_ENABLE_DIRECT_SSH_AIWG_PROXY=1`. |
| `aiwg_exec` | Runs allowlisted `aiwg` subcommands over the legacy SSH bridge. | Disabled by default; requires `AGENTIC_ENABLE_DIRECT_SSH_AIWG_PROXY=1`. |

## Required Naming And Audit Semantics

Use `legacy_direct_runtime_aiwg_proxy` when logging or discussing this path.
It is distinct from:

- provider/workload SSH credentials used by workload tooling;
- legacy direct-runtime dev/break-glass SSH keys used for VM diagnostics;
- future gateway SSH certificate leases from #531.

Any retained call path must log that it is using the legacy direct-runtime AIWG
proxy and must not log private key material, certificate material, command
secrets, or file contents.

## Gateway Replacement Requirements

#531 should replace this direct runtime bridge with gateway-mediated SSH
certificate or lease semantics:

- bind actor, instance id, command surface, access mode, and TTL into lease
  metadata;
- route through the gateway policy/audit boundary rather than host `ssh`
  directly to the runtime;
- emit audit events that distinguish manifest CRUD, AIWG exec, and SSH lease
  issuance outcomes;
- keep private key/cert material out of logs, env, session records, operation
  results, and PTY replay metadata;
- extend #533 leakage tests to cover the retained legacy proxy while it exists
  and the gateway replacement after #531 lands.

## Verification

- `cargo fmt --all --check` from `management/`
- `cargo test direct_ssh_aiwg_proxy_gate_is_opt_in --lib` from `management/`
- `cargo test direct_ssh_aiwg_proxy_disabled_response_names_legacy_path --lib`
  from `management/`
- `cargo test ensure_md_extension_appends_extension_once --lib` from
  `management/`
- Search for direct AIWG SSH proxy references confirms the retained code path is
  named as `legacy_direct_runtime_aiwg_proxy` and the public disposition says
  managed-profile SSH must use the gateway-mediated path.

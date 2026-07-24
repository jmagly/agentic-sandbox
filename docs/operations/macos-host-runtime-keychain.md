# macOS Host Runtime and Local CA Keychain Runbook

## Purpose

Configure the Apple Silicon native-host runtime as an explicitly enabled
per-user LaunchAgent and select the macOS Keychain backend for the workstation
CA root private key. The host runtime grants agents the ambient permissions of
the logged-in user; it has no VM or container isolation. Keychain reset and CA
rotation are destructive, witnessed human ceremonies. This procedure never
exports private key material.

## System Topology

| Item | Value |
| --- | --- |
| Supported host | Apple Silicon macOS |
| LaunchAgent label | `io.aiwg.agentic-sandbox.host-runtime` |
| Package binaries | `/usr/local/bin/agentic-host-runtime-daemon`, `/usr/local/bin/agent-client` |
| User state | `~/Library/Application Support/io.aiwg.agentic-sandbox/host-runtime` |
| Default workspace | `~/Library/Application Support/io.aiwg.agentic-sandbox/workspace` |
| Unix socket | `$TMPDIR/io.aiwg.agentic-sandbox/host-runtime.sock` |
| Local CA public state | `~/Library/Application Support/io.aiwg.agentic-sandbox/secrets/grpc-local-ca` |
| Keychain service | `io.aiwg.agentic-sandbox.grpc-local-ca` |
| Keychain account | `root-key:sandbox.agentic.local` by default |
| Isolation | Full daemon-user host access; no Linux cgroups, namespaces, seccomp, or systemd |

The root certificate remains a public PEM file. With
`AGENTIC_GRPC_LOCAL_CA_KEY_STORE=macos-keychain`, the matching root private key
exists only as a non-synchronizing generic-password item in the login
Keychain. Agent and server leaf keys retain their existing explicitly
configured file paths.

## Procedure

1. Build or obtain the Apple Silicon binaries. For a development build:

   ```bash
   cargo build --locked --release --manifest-path management/Cargo.toml \
     --no-default-features \
     --bin agentic-mgmt --bin agentic-host-runtime-daemon --bin grpc-local-ca
   cargo build --locked --release --manifest-path agent-rs/Cargo.toml
   ```

   Expected: both commands exit `0`.

2. Render the LaunchAgent. This resolves the current user's `HOME`, `TMPDIR`,
   private Application Support state/workspace paths, and short Unix-socket
   path into the plist because launchd does not inherit an interactive shell
   environment. Point it at installed binaries on the startup volume; do not
   run a persistent LaunchAgent directly from a removable or external build
   volume. This step does not install or load it:

   ```bash
   LAUNCH_AGENT="$HOME/Library/LaunchAgents/io.aiwg.agentic-sandbox.host-runtime.plist"
   mkdir -p "$HOME/Library/LaunchAgents"
   scripts/render-macos-launch-agent.sh \
     --daemon-binary "$PWD/management/target/release/agentic-host-runtime-daemon" \
     --agent-binary "$PWD/agent-rs/target/release/agent-client" \
     --output "$LAUNCH_AGENT"
   plutil -lint "$LAUNCH_AGENT"
   ```

   Expected:

   ```text
   rendered=$HOME/Library/LaunchAgents/io.aiwg.agentic-sandbox.host-runtime.plist
   ...: OK
   ```

3. Review the plist and explicitly enable the user service:

   ```bash
   LAUNCH_AGENT="$HOME/Library/LaunchAgents/io.aiwg.agentic-sandbox.host-runtime.plist"
   plutil -p "$LAUNCH_AGENT"
   launchctl bootstrap "gui/$(id -u)" "$LAUNCH_AGENT"
   ```

   Expected: `plutil` shows the fixed program arguments and `launchctl`
   returns no output. The service remains disabled until this explicit
   `bootstrap`.

4. Enable management-side daemon delegation and select Keychain storage before
   management's first local-CA start:

   ```bash
   export AGENTIC_HOST_RUNTIME_ENABLED=1
   export AGENTIC_HOST_RUNTIME_MODE=daemon
   export AGENTIC_GRPC_CA_BACKEND=local
   export AGENTIC_GRPC_LOCAL_CA_KEY_STORE=macos-keychain
   export AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_SERVICE=io.aiwg.agentic-sandbox.grpc-local-ca
   export AGENTIC_GRPC_LOCAL_CA_KEYCHAIN_ACCOUNT=root-key:sandbox.agentic.local
   ```

   Expected: no output. These values are identifiers, not credentials.

5. Start management from the same logged-in user session. The login Keychain
   must already be unlocked. In a headless or launchd context, management
   fails closed if Keychain access would require an interactive prompt.

   ```bash
   management/target/release/agentic-mgmt
   ```

   Expected log metadata includes:

   ```text
   backend="local" root_key_store="macos-keychain" gRPC mTLS CA backend configured
   ```

   It never includes the private key or a Keychain item value.

## Verification

1. Verify the LaunchAgent and private socket directory:

   ```bash
   LAUNCH_LABEL=io.aiwg.agentic-sandbox.host-runtime
   SOCKET_DIR="${TMPDIR%/}/io.aiwg.agentic-sandbox"
   launchctl print "gui/$(id -u)/$LAUNCH_LABEL" >/dev/null
   test "$(stat -f '%Lp' "$SOCKET_DIR")" = 700
   test -S "$SOCKET_DIR/host-runtime.sock"
   echo "launchd-host-runtime=pass"
   ```

   Expected:

   ```text
   launchd-host-runtime=pass
   ```

2. Verify the CA private key is present in Keychain without reading its value,
   and absent from the filesystem:

   ```bash
   KEYCHAIN_SERVICE=io.aiwg.agentic-sandbox.grpc-local-ca
   KEYCHAIN_ACCOUNT=root-key:sandbox.agentic.local
   CA_DIR="$HOME/Library/Application Support/io.aiwg.agentic-sandbox/secrets/grpc-local-ca"
   security find-generic-password \
     -s "$KEYCHAIN_SERVICE" -a "$KEYCHAIN_ACCOUNT" >/dev/null 2>&1
   test -s "$CA_DIR/grpc-local-root-ca.pem"
   test ! -e "$CA_DIR/grpc-local-root-ca-key.pem"
   echo "keychain-local-ca=pass"
   ```

   Expected:

   ```text
   keychain-local-ca=pass
   ```

3. Verify the public runtime warning:

   ```bash
   sandboxctl runtime list
   ```

   Expected: the `host` row reports isolation `full-host-access` and
   states that no VM or container isolation is enforced.

## Troubleshooting

- **Keychain item not found with an existing root certificate:** startup fails
  closed. Do not copy the filesystem key into Keychain automatically. Restore
  the original explicitly selected backend or perform the witnessed rotation
  procedure below.
- **Keychain locked or interaction not allowed:** unlock the login Keychain in
  the user session. Do not add a password to argv, environment variables,
  launchd plists, or CI. Headless startup must remain failed until
  non-interactive access is deliberately configured by an operator.
- **LaunchAgent will not bootstrap:** run `plutil -lint` and confirm both
  binaries are absolute Apple Silicon Mach-O paths. Do not substitute a
  system LaunchDaemon or root-owned runtime state.
- **Stale socket after an unclean exit:** confirm
  `launchctl print "gui/$(id -u)/io.aiwg.agentic-sandbox.host-runtime"` fails
  and no daemon process owns the socket before removing only
  `$TMPDIR/io.aiwg.agentic-sandbox/host-runtime.sock`.
- **Filesystem backend is still active:** confirm
  `AGENTIC_GRPC_LOCAL_CA_KEY_STORE=macos-keychain` is present in the
  management process environment. Selection is explicit and never inferred.

### Reset and rotation ceremony

Keychain reset or CA rotation invalidates all leaves issued by that root. A
human operator and witness must:

1. Stop new provisioning and stop management.
2. Record the old public root certificate fingerprint and affected instance
   identifiers. Never record private key material.
3. Confirm the exact Keychain service/account and public CA directory.
4. Manually delete the one Keychain item with
   `security delete-generic-password -s "$KEYCHAIN_SERVICE" -a "$KEYCHAIN_ACCOUNT"`.
5. Remove the matching public root certificate and stale leaf directories only
   after the operator confirms the blast radius and backup/re-enrollment plan.
6. Restart management with the same explicit Keychain configuration, record
   the new public fingerprint, and reprovision every affected agent.
7. Have both operator and witness sign the audit record.

Private-key export/import is intentionally unsupported. Migration uses a new
witnessed CA plus agent re-enrollment; it must never print, pipe, archive, or
temporarily write the old Keychain value.

## House Rules for Agents

- DO verify plist syntax and paths before any `launchctl bootstrap`.
- DO use unique synthetic Keychain service/account names in automated tests.
- DO remove only synthetic test items created by the same test.
- DO stop and report locked-Keychain or interaction-required failures.
- DON'T read a Keychain item value with `security ... -w`.
- DON'T export, import, migrate, print, or log private CA material.
- DON'T run the reset/rotation ceremony or delete a production Keychain item.
- DON'T install a system LaunchDaemon, enable root execution, or substitute
  root-owned runtime state.
- DON'T silently fall back from Keychain to filesystem storage.

## What NOT to Fix

- The root certificate remains on disk because it is public trust-bundle
  material; only the root private key moves to Keychain.
- The filesystem backend remains the default for compatibility. Keychain is
  explicit and never auto-selected or auto-migrated.
- A locked or prompt-requiring Keychain causes startup failure. This is the
  intended fail-closed behavior for SSH, CI, and unattended launchd contexts.
- Host runtime remains disabled until an operator explicitly loads the
  LaunchAgent and enables management-side host runtime selection.
- Apple `container`, Linux VM, VFIO, and GPU capabilities remain outside this
  native-host procedure.

## Audit Trail

| Field | Value |
| --- | --- |
| Procedure introduced | 2026-07-24 |
| Author | Agentic Sandbox maintainers |
| Last credential-free verification | Pending current-head mutsu validation |
| Applicable hosts | Apple Silicon macOS development/workstation hosts |
| Related issues | #495, #669, #676 |

Every real CA reset or rotation record must include UTC timestamp, operator,
witness, old and new public fingerprints, trust domain, affected instances,
and verification result. It must never include private key material.

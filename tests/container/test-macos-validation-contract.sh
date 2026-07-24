#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

workflow=.gitea/workflows/macos-validation.yml
validation=scripts/macos-validation.sh

# The remote login/session owns the location of its existing Rust toolchain.
# CI may extend that PATH, but must not replace it or pin a workstation path.
grep -Fq "export PATH=\"\$PATH:" "$workflow"
if grep -Fq '/Volumes/build/bt6' "$workflow"; then
  echo "macOS validation workflow must not pin mutsu's current toolchain volume" >&2
  exit 1
fi

# Prerequisites fail before the expensive build, and Docker errors stay
# sanitized rather than emitting daemon socket or user-directory details.
grep -Fq 'required_tools=(rustc cargo docker jq curl ditto file lsof launchctl pkgbuild pkgutil plutil shasum)' "$validation"
grep -Fq 'required mutsu validation tools are unavailable on PATH' "$validation"
grep -Fq 'the active Docker CLI context cannot reach a daemon' "$validation"
grep -Fq "docker version --format 'docker_client={{.Client.Version}} docker_server={{.Server.Version}} docker_arch={{.Server.Arch}}' 2>/dev/null" "$validation"

# Bootstrap enrollment carries the one-time credential over the dedicated
# server-authenticated TLS listener. The agent intentionally rejects HTTP.
grep -Fq 'for port in 48120 48122 48123 48124' "$validation"
grep -Fq 'AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL=https://host.docker.internal:48124/api/v1/bootstrap-enrollment/consume' "$validation"
grep -Fq -- '--dns-name host.docker.internal' "$validation"
grep -Fq -- '--dns-name localhost' "$validation"
grep -Fq -- '--grpc-tls-server-name localhost' "$validation"
grep -Fq -- '--bootstrap-enrollment-url https://localhost:48124/api/v1/bootstrap-enrollment/consume' "$validation"
grep -Fq -- "--bootstrap-ca \"\$scratch/management/secrets/grpc-local-ca/grpc-local-root-ca.pem\"" "$validation"
if grep -Fq 'AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL=http://' "$validation"; then
  echo "macOS validation must not send bootstrap enrollment over plaintext HTTP" >&2
  exit 1
fi

# The authorized Apple lane proves the native host tier with the same
# credential-free temporary CA and leaves no supervisor-owned state behind.
grep -Fq 'stage=native-host-secure-enrollment-task-session-lifecycle' "$validation"
grep -Fq 'stage=launchd-user-service-smoke' "$validation"
grep -Fq 'scripts/render-macos-launch-agent.sh' "$validation"
grep -Fq 'launchctl bootstrap "$launchd_domain" "$launchd_plist"' "$validation"
grep -Fq 'launchctl bootout "$launchd_domain/$launchd_label"' "$validation"
grep -Fq 'launchd_socket="$scratch/ld.sock"' "$validation"
grep -Fq 'launchd validation socket exceeds Darwin sockaddr_un.sun_path' "$validation"
grep -Fq 'launchd_bin_dir="$scratch/launchd-bin"' "$validation"
grep -Fq '"$launchd_bin_dir/agentic-host-runtime-daemon"' "$validation"
grep -Fq '"$launchd_bin_dir/agent-client"' "$validation"
grep -Fq 'for _ in {1..300}; do' "$validation"
grep -Fq 'StandardErrorPath' "$validation"
grep -Fq 'starting host runtime daemon' "$validation"
grep -Fq 'AGENTIC_RUN_MACOS_KEYCHAIN_TEST=1' "$validation"
grep -Fq 'CreateOptions::new()' management/src/grpc_local_ca.rs
grep -Fq 'MacosKeychainRootKeyStore::with_keychain' management/src/grpc_local_ca.rs
grep -Fq 'credential-free temporary file Keychain' docs/platform-support.md
grep -Fq 'AGENT_SETUP_COMPLETE' "$repo_root/management/src/host_runtime.rs"
grep -Fq "'{name:\$name,runtime:\"host\",agentshare:false,start:true,working_dir:\$working_dir}'" "$validation"
grep -Fq 'host bootstrap token remained after enrollment' "$validation"
grep -Fq 'host_workspace_canonical=' "$validation"
grep -Fq 'stage=native-host-multi-instance-session-ownership' "$validation"
grep -Fq 'native host instances collided on agent identity' "$validation"
grep -Fq 'peer native host PTY session did not write working-directory evidence' "$validation"
grep -Fq 'stage=native-host-daemon-restart-durable-metadata' "$validation"
grep -Fq 'host daemon socket remained after daemon shutdown' "$validation"
grep -Fq 'stage=native-host-management-restart-reconciliation' "$validation"
grep -Fq '.runtime == "host" and .agent_registered == true and .agent_ready == true' "$validation"
grep -Fq 'post-restart native host task did not complete' "$validation"
grep -Fq 'native host stop request returned HTTP' "$validation"
grep -Fq 'native host agent remained alive after stop' "$validation"
grep -Fq 'native host state remained after destroy' "$validation"
if grep -Fq 'skip=native host enrollment' "$validation"; then
  echo "authorized native-host validation must not remain skipped" >&2
  exit 1
fi

# Credential-free packaging contains the complete supported runtime surface,
# validates install/uninstall in an isolated root, and never activates launchd.
grep -Fq 'stage=credential-free-full-package-install-uninstall' "$validation"
grep -Fq 'scripts/package-macos.sh \' "$validation"
grep -Fq -- '--mode preview' "$validation"
grep -Fq 'scripts/smoke-macos-package.sh \' "$validation"
grep -Fq 'agentic-host-runtime-daemon' scripts/package-macos.sh
grep -Fq 'agentic-mgmt' scripts/package-macos.sh
grep -Fq 'uninstall-macos' scripts/package-macos.sh
grep -Fq 'launchd/io.aiwg.agentic-sandbox.host-runtime.plist' scripts/package-macos.sh
for package_path in \
  deploy/packaging/macos \
  scripts/package-macos.sh \
  scripts/smoke-macos-package.sh \
  scripts/uninstall-macos.sh \
  tests/package/test-package-macos.sh; do
  grep -Fq "$package_path" .gitea/workflows/macos-validation.yml \
    || { echo "macOS validation trigger omits $package_path" >&2; exit 1; }
done
if grep -Eq 'launchctl (bootstrap|load|enable|kickstart)' scripts/package-macos.sh; then
  echo "macOS package construction must not activate launchd" >&2
  exit 1
fi

grep -Fq 'full host access' deploy/launchd/io.aiwg.agentic-sandbox.host-runtime.plist
grep -Fq 'This command only renders a plist. It never installs, bootstraps, enables, or' \
  scripts/render-macos-launch-agent.sh
grep -Fq 'EnvironmentVariables.AGENTIC_HOST_RUNTIME_DAEMON_SOCKET' \
  scripts/render-macos-launch-agent.sh
grep -Fq 'EnvironmentVariables.HOME' scripts/render-macos-launch-agent.sh
grep -Fq 'rendered socket path exceeds Darwin sockaddr_un.sun_path' \
  scripts/render-macos-launch-agent.sh
grep -Fq 'Host runtime has full access as the daemon user' management/ui/index.html
grep -Fq 'Acknowledge the full-host-access warning before continuing' management/ui/app.js

echo "macOS validation contract: ok"

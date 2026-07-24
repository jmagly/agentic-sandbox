#!/usr/bin/env bash
# Native Apple Silicon build and bounded runtime smoke validation for mutsu.
set -euo pipefail

report="${AGENTIC_MACOS_REPORT:-macos-validation.txt}"
expected_sha="${AGENTIC_EXPECTED_SHA:-unknown}"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/agentic-macos-validation.XXXXXX")"
daemon_pid=""
mgmt_pid=""
image=""
container_name=""
instance_id=""
host_instance_id=""
host_peer_instance_id=""
launchd_label=""
launchd_domain=""
launchd_loaded=0

wait_operation() {
  local operation_id="$1"
  local operation_json=""
  local state=""
  for _ in {1..120}; do
    operation_json="$(curl -fsS "http://127.0.0.1:48122/api/v2/admin/operations/$operation_id")"
    state="$(jq -r '.state' <<<"$operation_json")"
    case "$state" in
      succeeded) printf '%s' "$operation_json"; return 0 ;;
      failed)
        jq '{id,kind,state,error}' <<<"$operation_json" >&2
        return 1
        ;;
    esac
    sleep 0.25
  done
  echo "FAIL: operation $operation_id did not reach a terminal state" >&2
  return 1
}

wait_instance_ready() {
  local wanted_id="$1"
  local wanted_runtime="$2"
  local instance_json=""
  for _ in {1..120}; do
    instance_json="$(curl -fsS "http://127.0.0.1:48122/api/v2/admin/instances?runtime=$wanted_runtime")"
    if jq -e --arg id "$wanted_id" --arg runtime "$wanted_runtime" \
      '.items | any(.id == $id and .runtime == $runtime and .agent_registered == true and .agent_ready == true and .transport == "mtls")' \
      <<<"$instance_json" >/dev/null; then
      return 0
    fi
    sleep 0.25
  done
  echo "FAIL: $wanted_runtime instance did not register as ready over mTLS" >&2
  return 1
}

start_management() {
  LISTEN_ADDR=127.0.0.1:48120 \
  SECRETS_DIR="$scratch/management/secrets" \
  AGENTSHARE_ROOT="$scratch/agentshare" \
  DOCKER_MONITOR_ENABLED=false \
  AGENTIC_GRPC_VSOCK_PORT=0 \
  AGENTIC_HTTP_LISTEN_IP=0.0.0.0 \
  AGENTIC_GRPC_MTLS_LISTEN=0.0.0.0:48123 \
  AGENTIC_GRPC_MTLS_CERT="$scratch/management/mtls-server/server.pem" \
  AGENTIC_GRPC_MTLS_KEY="$scratch/management/mtls-server/server-key.pem" \
  AGENTIC_GRPC_MTLS_CLIENT_CA="$scratch/management/secrets/grpc-local-ca/grpc-local-root-ca.pem" \
  AGENTIC_CONTAINER_GRPC_SERVER=host.docker.internal:48123 \
  AGENTIC_CONTAINER_BOOTSTRAP_ENROLLMENT_URL=https://host.docker.internal:48124/api/v1/bootstrap-enrollment/consume \
  AGENTIC_HOST_RUNTIME_ENABLED=1 \
  AGENTIC_HOST_RUNTIME_MODE=daemon \
  AGENTIC_HOST_RUNTIME_DAEMON_SOCKET="$scratch/host-runtime.sock" \
  management/target/release/agentic-mgmt &
  mgmt_pid=$!
  for _ in {1..100}; do
    if curl -fsS http://127.0.0.1:48122/healthz >/dev/null 2>&1; then
      return 0
    fi
    kill -0 "$mgmt_pid" 2>/dev/null || {
      echo "FAIL: management server exited before health became ready"
      return 1
    }
    sleep 0.1
  done
  echo "FAIL: management server health did not become ready"
  return 1
}

start_host_daemon() {
  management/target/release/agentic-host-runtime-daemon \
    --socket "$scratch/host-runtime.sock" \
    --root-dir "$scratch/host-state" \
    --agent-client "$PWD/agent-rs/target/release/agent-client" \
    --management-server 127.0.0.1:48123 \
    --grpc-tls-server-name localhost \
    --bootstrap-enrollment-url https://localhost:48124/api/v1/bootstrap-enrollment/consume \
    --bootstrap-ca "$scratch/management/secrets/grpc-local-ca/grpc-local-root-ca.pem" \
    --socket-mode 600 &
  daemon_pid=$!
  for _ in {1..50}; do
    [[ -S "$scratch/host-runtime.sock" ]] && return 0
    kill -0 "$daemon_pid" 2>/dev/null || {
      echo "FAIL: host daemon exited before binding"
      return 1
    }
    sleep 0.1
  done
  echo "FAIL: host daemon socket was not created"
  return 1
}

cleanup() {
  if [[ "$launchd_loaded" == "1" ]] && [[ -n "$launchd_domain" ]] && [[ -n "$launchd_label" ]]; then
    launchctl bootout "$launchd_domain/$launchd_label" >/dev/null 2>&1 || true
    launchd_loaded=0
  fi
  if [[ -n "$host_instance_id" ]] && [[ -n "$mgmt_pid" ]] && kill -0 "$mgmt_pid" 2>/dev/null; then
    curl -fsS -X POST \
      "http://127.0.0.1:48122/api/v2/admin/instances/$host_instance_id/destroy" \
      >/dev/null 2>&1 || true
  fi
  if [[ -n "$host_peer_instance_id" ]] && [[ -n "$mgmt_pid" ]] && kill -0 "$mgmt_pid" 2>/dev/null; then
    curl -fsS -X POST \
      "http://127.0.0.1:48122/api/v2/admin/instances/$host_peer_instance_id/destroy" \
      >/dev/null 2>&1 || true
  fi
  if [[ -n "$instance_id" ]] && [[ -n "$mgmt_pid" ]] && kill -0 "$mgmt_pid" 2>/dev/null; then
    curl -fsS -X POST \
      "http://127.0.0.1:48122/api/v2/admin/instances/$instance_id/destroy" \
      >/dev/null 2>&1 || true
  fi
  if [[ -n "$container_name" ]]; then
    docker rm -f "$container_name" >/dev/null 2>&1 || true
  fi
  if [[ -n "$mgmt_pid" ]] && kill -0 "$mgmt_pid" 2>/dev/null; then
    kill -TERM "$mgmt_pid" 2>/dev/null || true
    wait "$mgmt_pid" 2>/dev/null || true
  fi
  if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -f "$scratch/host-runtime.sock"
  if [[ -n "$image" ]]; then docker image rm "$image" >/dev/null 2>&1 || true; fi
  rm -rf "$scratch"
}
trap cleanup EXIT INT TERM

exec > >(tee "$report") 2>&1
echo "schema=agentic.macos-validation.v1"
echo "commit=$expected_sha"
echo "timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "architecture=$(uname -m)"
echo "macos=$(sw_vers -productVersion)"
echo "darwin=$(uname -r)"

required_tools=(rustc cargo docker jq curl ditto file lsof launchctl pkgbuild pkgutil plutil shasum)
missing_tools=()
for tool in "${required_tools[@]}"; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing_tools+=("$tool")
  fi
done
if ((${#missing_tools[@]})); then
  printf 'FAIL: required mutsu validation tools are unavailable on PATH:' >&2
  printf ' %s' "${missing_tools[@]}" >&2
  printf '\n' >&2
  exit 1
fi

rustc --version
cargo --version

[[ "$(uname -m)" == "arm64" ]] || { echo "FAIL: mutsu must report arm64"; exit 1; }

echo "stage=darwin-build"
cargo build --locked --release --manifest-path management/Cargo.toml --no-default-features \
  --bin agentic-mgmt --bin agentic-host-runtime-daemon --bin grpc-local-ca
cargo build --locked --release --manifest-path agent-rs/Cargo.toml
cargo build --locked --release --manifest-path cli/Cargo.toml --bin sandboxctl
cargo test --locked --manifest-path management/Cargo.toml --no-default-features --lib \
  portable_runtime_discovery_never_advertises_linux_vm_capabilities
cargo test --locked --manifest-path management/Cargo.toml --no-default-features --lib host_runtime::tests
AGENTIC_RUN_MACOS_KEYCHAIN_TEST=1 \
  cargo test --locked --manifest-path management/Cargo.toml --no-default-features --lib \
  grpc_local_ca::tests::synthetic_macos_keychain_round_trip_and_spiffe_continuity \
  -- --exact

for binary in \
  management/target/release/agentic-mgmt \
  management/target/release/agentic-host-runtime-daemon \
  management/target/release/grpc-local-ca \
  agent-rs/target/release/agent-client \
  cli/target/release/sandboxctl; do
  file "$binary"
  file "$binary" | grep -q 'arm64' || { echo "FAIL: $binary is not arm64"; exit 1; }
done

echo "stage=credential-free-full-package-install-uninstall"
package_source="$scratch/package-source"
package_output="$scratch/package-output"
package_install_root="$scratch/package-install-root"
mkdir -p "$package_source" "$package_output"
install -m 0755 management/target/release/agentic-mgmt "$package_source/agentic-mgmt"
install -m 0755 \
  management/target/release/agentic-host-runtime-daemon \
  "$package_source/agentic-host-runtime-daemon"
install -m 0755 cli/target/release/sandboxctl "$package_source/sandboxctl"
install -m 0755 agent-rs/target/release/agent-client "$package_source/agent-client"
package_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' management/Cargo.toml | head -n 1)"
[[ -n "$package_version" ]] || {
  echo "FAIL: management package version could not be resolved"
  exit 1
}
scripts/package-macos.sh \
  --mode preview \
  --version "$package_version" \
  --source-dir "$package_source" \
  --out-dir "$package_output"
preview_package="$package_output/agentic-sandbox-v${package_version}-aarch64-darwin-preview.pkg"
scripts/smoke-macos-package.sh \
  --package "$preview_package" \
  --install-root "$package_install_root"
[[ ! -e "$package_install_root/usr/local/bin/agentic-mgmt" ]] || {
  echo "FAIL: isolated package uninstall left management installed"
  exit 1
}
[[ ! -e "$package_install_root/usr/local/bin/agentic-host-runtime-daemon" ]] || {
  echo "FAIL: isolated package uninstall left the host daemon installed"
  exit 1
}

mkdir -p "$scratch/management/mtls-server" "$scratch/agentshare"
management/target/release/grpc-local-ca issue-server \
  --ca-dir "$scratch/management/secrets/grpc-local-ca" \
  --trust-domain sandbox.agentic.local \
  --dns-name host.docker.internal \
  --dns-name localhost \
  --cert "$scratch/management/mtls-server/server.pem" \
  --key "$scratch/management/mtls-server/server-key.pem" \
  >/dev/null
chmod 0600 "$scratch/management/mtls-server/server-key.pem"

echo "stage=launchd-user-service-smoke"
launchd_label="io.aiwg.agentic-sandbox.validation.${expected_sha:0:8}.$$"
launchd_domain="gui/$UID"
launchd_plist="$scratch/$launchd_label.plist"
launchd_socket="$scratch/ld.sock"
launchd_stdout="$scratch/launchd.stdout.log"
launchd_stderr="$scratch/launchd.stderr.log"
launchd_bin_dir="$scratch/launchd-bin"
mkdir -m 0700 "$launchd_bin_dir"
install -m 0755 \
  management/target/release/agentic-host-runtime-daemon \
  "$launchd_bin_dir/agentic-host-runtime-daemon"
install -m 0755 \
  agent-rs/target/release/agent-client \
  "$launchd_bin_dir/agent-client"
scripts/render-macos-launch-agent.sh \
  --daemon-binary "$launchd_bin_dir/agentic-host-runtime-daemon" \
  --agent-binary "$launchd_bin_dir/agent-client" \
  --output "$launchd_plist" \
  >/dev/null
plutil -replace Label -string "$launchd_label" "$launchd_plist"
plutil -replace EnvironmentVariables.AGENTIC_HOST_RUNTIME_DAEMON_SOCKET \
  -string "$launchd_socket" "$launchd_plist"
plutil -replace EnvironmentVariables.AGENTIC_HOST_RUNTIME_ROOT \
  -string "$scratch/launchd/state" "$launchd_plist"
plutil -replace EnvironmentVariables.AGENTIC_HOST_WORKSPACE_ROOT \
  -string "$scratch/launchd/workspace" "$launchd_plist"
plutil -insert StandardOutPath -string "$launchd_stdout" "$launchd_plist"
plutil -insert StandardErrorPath -string "$launchd_stderr" "$launchd_plist"
(( ${#launchd_socket} < 104 )) || {
  echo "FAIL: launchd validation socket exceeds Darwin sockaddr_un.sun_path"
  exit 1
}
plutil -lint "$launchd_plist"
launchctl bootstrap "$launchd_domain" "$launchd_plist"
launchd_loaded=1
for _ in {1..300}; do
  [[ -S "$launchd_socket" ]] && break
  sleep 0.1
done
[[ -S "$launchd_socket" ]] || {
  echo "FAIL: launchd host runtime daemon did not create its isolated socket"
  launchctl print "$launchd_domain/$launchd_label" || true
  [[ ! -s "$launchd_stdout" ]] || tail -n 100 "$launchd_stdout"
  [[ ! -s "$launchd_stderr" ]] || tail -n 100 "$launchd_stderr"
  exit 1
}
if ! grep -Fq 'starting host runtime daemon' "$launchd_stdout" "$launchd_stderr"; then
  echo "FAIL: launchd host runtime daemon did not emit its startup record"
  exit 1
fi
[[ "$(stat -f '%Lp' "$scratch")" == "700" ]] || {
  echo "FAIL: launchd host runtime socket directory is not mode 0700"
  exit 1
}
launchctl print "$launchd_domain/$launchd_label" >/dev/null
launchctl bootout "$launchd_domain/$launchd_label"
launchd_loaded=0
for _ in {1..50}; do
  [[ ! -e "$launchd_socket" ]] && break
  sleep 0.1
done
[[ ! -e "$launchd_socket" ]] || {
  echo "FAIL: launchd host runtime socket remained after bootout"
  exit 1
}

echo "stage=host-daemon-socket-smoke"
start_host_daemon

echo "stage=management-health-and-runtime-discovery"
for port in 48120 48122 48123 48124; do
  if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "FAIL: validation port $port is already in use; no process was changed"
    exit 1
  fi
done
start_management
curl -fsS http://127.0.0.1:48122/healthz >/dev/null
runtime_json="$(curl -fsS http://127.0.0.1:48122/api/v2/admin/runtime/providers)"
jq -e '.runtimes | any(.id == "host" and .available == true)' <<<"$runtime_json" >/dev/null
jq -e '.runtimes | any(.id == "docker")' <<<"$runtime_json" >/dev/null
jq -e '.runtimes | any(.id == "qemu" and .available == false and .unavailable_code == "qemu.platform_unsupported")' <<<"$runtime_json" >/dev/null

echo "stage=native-host-secure-enrollment-task-session-lifecycle"
host_workspace="$scratch/host-workspace"
mkdir -p "$host_workspace"
host_instance_name="macos-host-${expected_sha:0:8}-$$"
host_provision_json="$(jq -nc \
  --arg name "$host_instance_name" \
  --arg working_dir "$host_workspace" \
  '{name:$name,runtime:"host",agentshare:false,start:true,working_dir:$working_dir}')"
host_operation_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  --data "$host_provision_json" \
  http://127.0.0.1:48122/api/v2/admin/instances | jq -er '.id')"
host_operation_json="$(wait_operation "$host_operation_id")"
host_instance_id="$(jq -er '.result.instance_id' <<<"$host_operation_json")"
host_agent_id="$(jq -er '.result.watch_agents[0]' <<<"$host_operation_json")"
wait_instance_ready "$host_instance_id" host

host_instance_dir="$scratch/host-state/instances/$host_instance_id"
host_agent_env="$host_instance_dir/agent.env"
host_tls_dir="$host_instance_dir/tls"
[[ -f "$host_agent_env" ]] || { echo "FAIL: host agent env was not created"; exit 1; }
if grep -q '^AGENT_BOOTSTRAP_TOKEN=' "$host_agent_env"; then
  echo "FAIL: host bootstrap token remained after enrollment"
  exit 1
fi
for tls_file in ca.pem agent.pem agent-key.pem; do
  [[ -s "$host_tls_dir/$tls_file" ]] || {
    echo "FAIL: host enrollment did not materialize $tls_file"
    exit 1
  }
done

echo "stage=native-host-multi-instance-session-ownership"
host_peer_workspace="$scratch/host-peer-workspace"
mkdir -p "$host_peer_workspace"
host_peer_name="macos-host-peer-${expected_sha:0:8}-$$"
host_peer_provision_json="$(jq -nc \
  --arg name "$host_peer_name" \
  --arg working_dir "$host_peer_workspace" \
  '{name:$name,runtime:"host",agentshare:false,start:true,working_dir:$working_dir}')"
host_peer_operation_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  --data "$host_peer_provision_json" \
  http://127.0.0.1:48122/api/v2/admin/instances | jq -er '.id')"
host_peer_operation_json="$(wait_operation "$host_peer_operation_id")"
host_peer_instance_id="$(jq -er '.result.instance_id' <<<"$host_peer_operation_json")"
host_peer_agent_id="$(jq -er '.result.watch_agents[0]' <<<"$host_peer_operation_json")"
wait_instance_ready "$host_peer_instance_id" host
[[ "$host_peer_instance_id" != "$host_instance_id" ]] || {
  echo "FAIL: native host instances collided on instance identity"
  exit 1
}
[[ "$host_peer_agent_id" != "$host_agent_id" ]] || {
  echo "FAIL: native host instances collided on agent identity"
  exit 1
}
host_peer_instance_dir="$scratch/host-state/instances/$host_peer_instance_id"
host_pid="$(jq -er '.pid' "$host_instance_dir/metadata.json")"
host_peer_pid="$(jq -er '.pid' "$host_peer_instance_dir/metadata.json")"
[[ "$host_peer_pid" != "$host_pid" ]] || {
  echo "FAIL: native host instances collided on process identity"
  exit 1
}

host_task_marker="macos-host-task-${expected_sha:0:12}"
host_task_json="$(jq -nc --arg marker "$host_task_marker" \
  '{message:{role:"user",parts:[{kind:"text",text:$marker}]}}')"
host_task_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  -H 'A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1, https://agentic-sandbox.aiwg.io/extensions/idempotency/v1' \
  --data "$host_task_json" \
  "http://127.0.0.1:48122/agents/$host_instance_id/v1/messages:send" | jq -er '.id')"
for _ in {1..120}; do
  host_task_state="$(curl -fsS \
    "http://127.0.0.1:48122/agents/$host_instance_id/v1/tasks/$host_task_id" | jq -r '.status.state')"
  [[ "$host_task_state" == "completed" ]] && break
  [[ "$host_task_state" == "failed" || "$host_task_state" == "canceled" || "$host_task_state" == "rejected" ]] && {
    echo "FAIL: native host task entered terminal state $host_task_state"
    exit 1
  }
  sleep 0.25
done
[[ "$host_task_state" == "completed" ]] || { echo "FAIL: native host task did not complete"; exit 1; }
host_artifacts_json="$(curl -fsS \
  "http://127.0.0.1:48122/agents/$host_instance_id/v1/tasks/$host_task_id/artifacts")"
jq -e --arg expected "$host_task_marker" \
  '.artifacts | any(.artifact.stream == "stdout" and (.artifact.data | rtrimstr("\n")) == $expected)' \
  <<<"$host_artifacts_json" >/dev/null

host_session_name="macos-host-workspace-${expected_sha:0:8}"
host_workspace_canonical="$(cd "$host_workspace" && pwd -P)"
host_session_json="$(jq -nc \
  --arg name "$host_session_name" \
  --arg working_dir "$host_workspace" \
  '{command:"sh",args:["-c","pwd -P > .macos-host-session-proof; sleep 120"],working_dir:$working_dir,session_name:$name}')"
host_session_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  --data "$host_session_json" \
  "http://127.0.0.1:48122/api/v1/agents/$host_agent_id/sessions" | jq -er '.session_id')"
for _ in {1..40}; do
  host_session_list="$(curl -fsS \
    "http://127.0.0.1:48122/api/v1/agents/$host_agent_id/sessions")"
  jq -e --arg id "$host_session_id" '.sessions | any(.session_id == $id)' \
    <<<"$host_session_list" >/dev/null && break
  sleep 0.25
done
jq -e --arg id "$host_session_id" '.sessions | any(.session_id == $id)' \
  <<<"$host_session_list" >/dev/null
host_workspace_proof="$host_workspace/.macos-host-session-proof"
for _ in {1..40}; do
  [[ -f "$host_workspace_proof" ]] && break
  sleep 0.25
done
[[ -f "$host_workspace_proof" ]] || {
  echo "FAIL: native host PTY session did not write working-directory evidence"
  exit 1
}
[[ "$(tr -d '\r\n' < "$host_workspace_proof")" == "$host_workspace_canonical" ]] || {
  echo "FAIL: native host PTY session did not use the requested working directory"
  exit 1
}

host_peer_session_name="macos-host-peer-${expected_sha:0:8}"
host_peer_workspace_canonical="$(cd "$host_peer_workspace" && pwd -P)"
host_peer_session_json="$(jq -nc \
  --arg name "$host_peer_session_name" \
  --arg working_dir "$host_peer_workspace" \
  '{command:"sh",args:["-c","pwd -P > .macos-host-peer-session-proof; sleep 120"],working_dir:$working_dir,session_name:$name}')"
host_peer_session_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  --data "$host_peer_session_json" \
  "http://127.0.0.1:48122/api/v1/agents/$host_peer_agent_id/sessions" | jq -er '.session_id')"
for _ in {1..40}; do
  host_peer_session_list="$(curl -fsS \
    "http://127.0.0.1:48122/api/v1/agents/$host_peer_agent_id/sessions")"
  jq -e --arg id "$host_peer_session_id" '.sessions | any(.session_id == $id)' \
    <<<"$host_peer_session_list" >/dev/null && break
  sleep 0.25
done
jq -e --arg id "$host_peer_session_id" '.sessions | any(.session_id == $id)' \
  <<<"$host_peer_session_list" >/dev/null
jq -e --arg id "$host_peer_session_id" '.sessions | all(.session_id != $id)' \
  <<<"$host_session_list" >/dev/null
host_peer_workspace_proof="$host_peer_workspace/.macos-host-peer-session-proof"
for _ in {1..40}; do
  [[ -f "$host_peer_workspace_proof" ]] && break
  sleep 0.25
done
[[ -f "$host_peer_workspace_proof" ]] || {
  echo "FAIL: peer native host PTY session did not write working-directory evidence"
  exit 1
}
[[ "$(tr -d '\r\n' < "$host_peer_workspace_proof")" == "$host_peer_workspace_canonical" ]] || {
  echo "FAIL: peer native host PTY session did not use its requested working directory"
  exit 1
}

echo "stage=native-host-daemon-restart-durable-metadata"
kill -TERM "$daemon_pid"
wait "$daemon_pid" || true
daemon_pid=""
for _ in {1..50}; do
  [[ ! -e "$scratch/host-runtime.sock" ]] && break
  sleep 0.1
done
[[ ! -e "$scratch/host-runtime.sock" ]] || {
  echo "FAIL: host daemon socket remained after daemon shutdown"
  exit 1
}
kill -0 "$host_pid"
kill -0 "$host_peer_pid"
start_host_daemon

echo "stage=native-host-management-restart-reconciliation"
kill -TERM "$mgmt_pid"
wait "$mgmt_pid" || true
mgmt_pid=""
start_management
wait_instance_ready "$host_instance_id" host
wait_instance_ready "$host_peer_instance_id" host
reconciled_instances="$(curl -fsS \
  "http://127.0.0.1:48122/api/v2/admin/instances?runtime=host")"
jq -e --arg id "$host_instance_id" \
  '.items | any(.id == $id and .runtime == "host" and .agent_registered == true and .agent_ready == true)' \
  <<<"$reconciled_instances" >/dev/null
jq -e --arg id "$host_peer_instance_id" \
  '.items | any(.id == $id and .runtime == "host" and .agent_registered == true and .agent_ready == true)' \
  <<<"$reconciled_instances" >/dev/null
for _ in {1..40}; do
  host_session_list="$(curl -fsS \
    "http://127.0.0.1:48122/api/v1/agents/$host_agent_id/sessions")"
  jq -e --arg id "$host_session_id" '.sessions | any(.session_id == $id)' \
    <<<"$host_session_list" >/dev/null && break
  sleep 0.25
done
jq -e --arg id "$host_session_id" '.sessions | any(.session_id == $id)' \
  <<<"$host_session_list" >/dev/null
for _ in {1..40}; do
  host_peer_session_list="$(curl -fsS \
    "http://127.0.0.1:48122/api/v1/agents/$host_peer_agent_id/sessions")"
  jq -e --arg id "$host_peer_session_id" '.sessions | any(.session_id == $id)' \
    <<<"$host_peer_session_list" >/dev/null && break
  sleep 0.25
done
jq -e --arg id "$host_peer_session_id" '.sessions | any(.session_id == $id)' \
  <<<"$host_peer_session_list" >/dev/null
jq -e --arg id "$host_peer_session_id" '.sessions | all(.session_id != $id)' \
  <<<"$host_session_list" >/dev/null
jq -e --arg id "$host_session_id" '.sessions | all(.session_id != $id)' \
  <<<"$host_peer_session_list" >/dev/null

host_restart_marker="macos-host-restart-${expected_sha:0:12}"
host_restart_task_json="$(jq -nc --arg marker "$host_restart_marker" \
  '{message:{role:"user",parts:[{kind:"text",text:$marker}]}}')"
host_restart_task_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  -H 'A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1, https://agentic-sandbox.aiwg.io/extensions/idempotency/v1' \
  --data "$host_restart_task_json" \
  "http://127.0.0.1:48122/agents/$host_instance_id/v1/messages:send" | jq -er '.id')"
for _ in {1..120}; do
  host_restart_task_state="$(curl -fsS \
    "http://127.0.0.1:48122/agents/$host_instance_id/v1/tasks/$host_restart_task_id" \
    | jq -r '.status.state')"
  [[ "$host_restart_task_state" == "completed" ]] && break
  [[ "$host_restart_task_state" == "failed" || "$host_restart_task_state" == "canceled" || "$host_restart_task_state" == "rejected" ]] && {
    echo "FAIL: post-restart native host task entered terminal state $host_restart_task_state"
    exit 1
  }
  sleep 0.25
done
[[ "$host_restart_task_state" == "completed" ]] || {
  echo "FAIL: post-restart native host task did not complete"
  exit 1
}

curl -fsS -X DELETE \
  "http://127.0.0.1:48122/api/v1/sessions/$host_session_id?signal=TERM" >/dev/null
curl -fsS -X DELETE \
  "http://127.0.0.1:48122/api/v1/sessions/$host_peer_session_id?signal=TERM" >/dev/null

host_agent_pid="$(jq -er '.pid' "$host_instance_dir/metadata.json")"
host_stop_response="$(curl -sS -X POST \
  -w $'\n%{http_code}' \
  "http://127.0.0.1:48122/api/v2/admin/instances/$host_instance_id/stop")"
host_stop_status="${host_stop_response##*$'\n'}"
host_stop_body="${host_stop_response%$'\n'*}"
if [[ ! "$host_stop_status" =~ ^2[0-9][0-9]$ ]]; then
  host_stop_detail="$(jq -r '.error.detail // .detail // .message // "unspecified supervisor error"' \
    <<<"$host_stop_body" 2>/dev/null || printf 'unparseable supervisor error')"
  echo "FAIL: native host stop request returned HTTP $host_stop_status: $host_stop_detail"
  exit 1
fi
host_stop_operation="$(jq -er '.id' <<<"$host_stop_body")"
wait_operation "$host_stop_operation" >/dev/null
for _ in {1..50}; do
  kill -0 "$host_agent_pid" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$host_agent_pid" 2>/dev/null; then
  echo "FAIL: native host agent remained alive after stop"
  exit 1
fi
host_destroy_operation="$(curl -fsS -X POST \
  "http://127.0.0.1:48122/api/v2/admin/instances/$host_instance_id/destroy" | jq -er '.id')"
wait_operation "$host_destroy_operation" >/dev/null
[[ ! -e "$host_instance_dir" ]] || {
  echo "FAIL: native host state remained after destroy"
  exit 1
}
host_instance_id=""

host_peer_agent_pid="$(jq -er '.pid' "$host_peer_instance_dir/metadata.json")"
host_peer_stop_operation="$(curl -fsS -X POST \
  "http://127.0.0.1:48122/api/v2/admin/instances/$host_peer_instance_id/stop" | jq -er '.id')"
wait_operation "$host_peer_stop_operation" >/dev/null
for _ in {1..50}; do
  kill -0 "$host_peer_agent_pid" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$host_peer_agent_pid" 2>/dev/null; then
  echo "FAIL: peer native host agent remained alive after stop"
  exit 1
fi
host_peer_destroy_operation="$(curl -fsS -X POST \
  "http://127.0.0.1:48122/api/v2/admin/instances/$host_peer_instance_id/destroy" | jq -er '.id')"
wait_operation "$host_peer_destroy_operation" >/dev/null
[[ ! -e "$host_peer_instance_dir" ]] || {
  echo "FAIL: peer native host state remained after destroy"
  exit 1
}
host_peer_instance_id=""

echo "stage=docker-desktop-arm64-smoke"
if ! docker_version="$(docker version --format 'docker_client={{.Client.Version}} docker_server={{.Server.Version}} docker_arch={{.Server.Arch}}' 2>/dev/null)"; then
  echo "FAIL: the active Docker CLI context cannot reach a daemon; start Docker Desktop or select an already-running Docker Desktop context" >&2
  exit 1
fi
printf '%s\n' "$docker_version"
image="agentic/macos-validation:${expected_sha:0:12}"
docker build --platform linux/arm64 -f images/container/Dockerfile.base -t "$image" .
docker run --rm --platform linux/arm64 --entrypoint /bin/sh "$image" -c \
  'test "$(uname -m)" = aarch64; test -x /usr/local/bin/agent-client'

echo "stage=docker-secure-enrollment-task-session-lifecycle"
container_name="macos-validation-${expected_sha:0:8}-$$"
provision_json="$(jq -nc \
  --arg name "$container_name" \
  --arg image "$image" \
  '{name:$name,runtime:"docker",image:$image,agentshare:true,start:true}')"
operation_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  --data "$provision_json" \
  http://127.0.0.1:48122/api/v2/admin/instances | jq -er '.id')"
operation_json="$(wait_operation "$operation_id")"
instance_id="$(jq -er '.result.instance_id' <<<"$operation_json")"
wait_instance_ready "$instance_id" docker

task_marker="macos-docker-task-${expected_sha:0:12}"
task_json="$(jq -nc --arg marker "$task_marker" \
  '{message:{role:"user",parts:[{kind:"text",text:$marker}]}}')"
task_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  -H 'A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1, https://agentic-sandbox.aiwg.io/extensions/idempotency/v1' \
  --data "$task_json" \
  "http://127.0.0.1:48122/agents/$instance_id/v1/messages:send" | jq -er '.id')"
for _ in {1..120}; do
  task_state="$(curl -fsS \
    "http://127.0.0.1:48122/agents/$instance_id/v1/tasks/$task_id" | jq -r '.status.state')"
  [[ "$task_state" == "completed" ]] && break
  [[ "$task_state" == "failed" || "$task_state" == "canceled" || "$task_state" == "rejected" ]] && {
    echo "FAIL: Docker task entered terminal state $task_state"
    exit 1
  }
  sleep 0.25
done
[[ "$task_state" == "completed" ]] || { echo "FAIL: Docker task did not complete"; exit 1; }
artifacts_json="$(curl -fsS \
  "http://127.0.0.1:48122/agents/$instance_id/v1/tasks/$task_id/artifacts")"
jq -e --arg expected "$task_marker" \
  '.artifacts | any(.artifact.stream == "stdout" and (.artifact.data | rtrimstr("\n")) == $expected)' \
  <<<"$artifacts_json" >/dev/null

session_name="macos-workspace-${expected_sha:0:8}"
session_json="$(jq -nc --arg name "$session_name" \
  '{command:"sh",args:["-c","pwd > /workspace/.macos-session-proof; sleep 30"],working_dir:"/workspace",session_name:$name}')"
session_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  --data "$session_json" \
  "http://127.0.0.1:48122/api/v1/agents/$container_name/sessions" | jq -er '.session_id')"
for _ in {1..40}; do
  session_list="$(curl -fsS \
    "http://127.0.0.1:48122/api/v1/agents/$container_name/sessions")"
  jq -e --arg id "$session_id" '.sessions | any(.session_id == $id)' \
    <<<"$session_list" >/dev/null && break
  sleep 0.25
done
jq -e --arg id "$session_id" '.sessions | any(.session_id == $id)' \
  <<<"$session_list" >/dev/null
workspace_proof="$scratch/agentshare/instances/$instance_id/workspace/.macos-session-proof"
for _ in {1..40}; do
  [[ -f "$workspace_proof" ]] && break
  sleep 0.25
done
[[ -f "$workspace_proof" ]] || {
  echo "FAIL: PTY session did not write shared-workspace evidence"
  exit 1
}
[[ "$(tr -d '\r\n' < "$workspace_proof")" == "/workspace" ]] || {
  echo "FAIL: PTY session did not run in the requested shared workspace"
  exit 1
}
curl -fsS -X DELETE \
  "http://127.0.0.1:48122/api/v1/sessions/$session_id?signal=TERM" >/dev/null

stop_operation="$(curl -fsS -X POST \
  "http://127.0.0.1:48122/api/v2/admin/instances/$instance_id/stop" | jq -er '.id')"
wait_operation "$stop_operation" >/dev/null
[[ "$(docker inspect -f '{{.State.Running}}' "$container_name")" == "false" ]]
start_operation="$(curl -fsS -X POST \
  "http://127.0.0.1:48122/api/v2/admin/instances/$instance_id/start" | jq -er '.id')"
wait_operation "$start_operation" >/dev/null
wait_instance_ready "$instance_id" docker
restart_marker="macos-docker-restart-${expected_sha:0:12}"
restart_task_json="$(jq -nc --arg marker "$restart_marker" \
  '{message:{role:"user",parts:[{kind:"text",text:$marker}]}}')"
restart_task_id="$(curl -fsS -X POST \
  -H 'Content-Type: application/json' \
  -H 'A2A-Extensions: https://agentic-sandbox.aiwg.io/extensions/runtime/v1, https://agentic-sandbox.aiwg.io/extensions/idempotency/v1' \
  --data "$restart_task_json" \
  "http://127.0.0.1:48122/agents/$instance_id/v1/messages:send" | jq -er '.id')"
for _ in {1..120}; do
  restart_task_state="$(curl -fsS \
    "http://127.0.0.1:48122/agents/$instance_id/v1/tasks/$restart_task_id" | jq -r '.status.state')"
  [[ "$restart_task_state" == "completed" ]] && break
  [[ "$restart_task_state" == "failed" || "$restart_task_state" == "canceled" || "$restart_task_state" == "rejected" ]] && {
    echo "FAIL: post-restart Docker task entered terminal state $restart_task_state"
    exit 1
  }
  sleep 0.25
done
[[ "$restart_task_state" == "completed" ]] || {
  echo "FAIL: post-restart Docker task did not complete"
  exit 1
}
destroy_operation="$(curl -fsS -X POST \
  "http://127.0.0.1:48122/api/v2/admin/instances/$instance_id/destroy" | jq -er '.id')"
wait_operation "$destroy_operation" >/dev/null
if docker inspect "$container_name" >/dev/null 2>&1; then
  echo "FAIL: Docker container remained after destroy"
  exit 1
fi
container_name=""
instance_id=""
docker image rm "$image" >/dev/null
image=""

kill -TERM "$mgmt_pid"
wait "$mgmt_pid" || true
mgmt_pid=""

kill -TERM "$daemon_pid"
wait "$daemon_pid" || true
daemon_pid=""
[[ ! -e "$scratch/host-runtime.sock" ]] || { echo "FAIL: host daemon left its socket behind"; exit 1; }

echo "skip=libvirt,KVM,Cloud-Hypervisor,VFIO,GPU; Linux-only runtime capabilities"
echo "result=pass"

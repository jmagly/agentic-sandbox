#!/usr/bin/env bash
# Native Apple Silicon build and bounded runtime smoke validation for mutsu.
set -euo pipefail

report="${AGENTIC_MACOS_REPORT:-macos-validation.txt}"
expected_sha="${AGENTIC_EXPECTED_SHA:-unknown}"
scratch="$(mktemp -d "${TMPDIR:-/tmp}/agentic-macos-validation.XXXXXX")"
daemon_pid=""
mgmt_pid=""
image=""

cleanup() {
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
rustc --version
cargo --version
docker version --format 'docker_client={{.Client.Version}} docker_server={{.Server.Version}} docker_arch={{.Server.Arch}}'

[[ "$(uname -m)" == "arm64" ]] || { echo "FAIL: mutsu must report arm64"; exit 1; }

echo "stage=darwin-build"
cargo build --locked --release --manifest-path management/Cargo.toml --no-default-features \
  --bin agentic-mgmt --bin agentic-host-runtime-daemon
cargo build --locked --release --manifest-path agent-rs/Cargo.toml
cargo build --locked --release --manifest-path cli/Cargo.toml --bin sandboxctl
cargo test --locked --manifest-path management/Cargo.toml --no-default-features --lib \
  portable_runtime_discovery_never_advertises_linux_vm_capabilities
cargo test --locked --manifest-path management/Cargo.toml --no-default-features --lib host_runtime::tests

for binary in \
  management/target/release/agentic-mgmt \
  management/target/release/agentic-host-runtime-daemon \
  agent-rs/target/release/agent-client \
  cli/target/release/sandboxctl; do
  file "$binary"
  file "$binary" | grep -q 'arm64' || { echo "FAIL: $binary is not arm64"; exit 1; }
done

echo "stage=host-daemon-socket-smoke"
management/target/release/agentic-host-runtime-daemon \
  --socket "$scratch/host-runtime.sock" \
  --root-dir "$scratch/host-state" \
  --agent-client "$PWD/agent-rs/target/release/agent-client" \
  --socket-mode 600 &
daemon_pid=$!
for _ in {1..50}; do
  [[ -S "$scratch/host-runtime.sock" ]] && break
  kill -0 "$daemon_pid" 2>/dev/null || { echo "FAIL: host daemon exited before binding"; exit 1; }
  sleep 0.1
done
[[ -S "$scratch/host-runtime.sock" ]] || { echo "FAIL: host daemon socket was not created"; exit 1; }

echo "stage=management-health-and-runtime-discovery"
LISTEN_ADDR=127.0.0.1:48120 \
SECRETS_DIR="$scratch/management/secrets" \
DOCKER_MONITOR_ENABLED=false \
AGENTIC_GRPC_VSOCK_PORT=0 \
AGENTIC_HOST_RUNTIME_ENABLED=1 \
AGENTIC_HOST_RUNTIME_MODE=daemon \
AGENTIC_HOST_RUNTIME_DAEMON_SOCKET="$scratch/host-runtime.sock" \
management/target/release/agentic-mgmt &
mgmt_pid=$!
for _ in {1..100}; do
  if curl -fsS http://127.0.0.1:48122/healthz >/dev/null 2>&1; then break; fi
  kill -0 "$mgmt_pid" 2>/dev/null || { echo "FAIL: management server exited before health became ready"; exit 1; }
  sleep 0.1
done
curl -fsS http://127.0.0.1:48122/healthz >/dev/null
runtime_json="$(curl -fsS http://127.0.0.1:48122/api/v2/admin/runtime/providers)"
jq -e '.runtimes | any(.id == "host" and .available == true)' <<<"$runtime_json" >/dev/null
jq -e '.runtimes | any(.id == "docker" and .available == true)' <<<"$runtime_json" >/dev/null
jq -e '.runtimes | any(.id == "qemu" and .available == false and .unavailable_code == "qemu.platform_unsupported")' <<<"$runtime_json" >/dev/null
kill -TERM "$mgmt_pid"
wait "$mgmt_pid" || true
mgmt_pid=""

kill -TERM "$daemon_pid"
wait "$daemon_pid" || true
daemon_pid=""
[[ ! -e "$scratch/host-runtime.sock" ]] || { echo "FAIL: host daemon left its socket behind"; exit 1; }

echo "stage=docker-desktop-arm64-smoke"
image="agentic/macos-validation:${expected_sha:0:12}"
docker build --platform linux/arm64 -f images/container/Dockerfile.base -t "$image" .
docker run --rm --platform linux/arm64 --entrypoint /bin/sh "$image" -c \
  'test "$(uname -m)" = aarch64; test -x /usr/local/bin/agent-client'
docker image rm "$image" >/dev/null
image=""

echo "skip=secure host/container enrollment and task/session lifecycle; requires authorized #669 integration setup"
echo "skip=libvirt,KVM,Cloud-Hypervisor,VFIO,GPU; Linux-only runtime capabilities"
echo "result=pass"

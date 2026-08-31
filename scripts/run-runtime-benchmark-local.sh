#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
OUTPUT=""
SAMPLES=3
WARMUPS=1
DOCKER_IMAGE="ghcr.io/jmagly/agentic-sandbox-agent:v2026.7.20"
BASE_IMAGE="ubuntu-26.04"

usage() {
  cat <<'EOF'
usage: scripts/run-runtime-benchmark-local.sh --output DIR [options]

Options:
  --samples N          comparable samples per runtime (minimum 3; default 3)
  --warmups N          unretained warmup samples (default 1)
  --docker-image REF   pinned image containing Python 3
  --base-image NAME    provision-vm base image name (default ubuntu-26.04)

The run creates one disposable Docker container and two disposable 2-vCPU/2-GiB
VMs. Exact-name cleanup runs on success, failure, and interruption.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) OUTPUT="${2:?--output requires a directory}"; shift 2 ;;
    --samples) SAMPLES="${2:?--samples requires a value}"; shift 2 ;;
    --warmups) WARMUPS="${2:?--warmups requires a value}"; shift 2 ;;
    --docker-image) DOCKER_IMAGE="${2:?--docker-image requires a value}"; shift 2 ;;
    --base-image) BASE_IMAGE="${2:?--base-image requires a value}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$OUTPUT" ]] || { usage >&2; exit 2; }
[[ "$SAMPLES" =~ ^[0-9]+$ && "$SAMPLES" -ge 3 ]] || {
  echo "--samples must be an integer of at least 3" >&2
  exit 2
}
[[ "$WARMUPS" =~ ^[0-9]+$ ]] || {
  echo "--warmups must be a non-negative integer" >&2
  exit 2
}

for command in docker virsh jq python3 ssh ssh-keygen; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 1
  }
done
[[ -r /dev/kvm && -w /dev/kvm ]] || {
  echo "/dev/kvm is not readable and writable by the current user" >&2
  exit 1
}
timeout 10 docker info >/dev/null
timeout 10 virsh -c qemu:///system version >/dev/null
[[ -x /opt/agentic-sandbox/cloud-hypervisor/current/bin/cloud-hypervisor ]] || {
  echo "the pinned Cloud Hypervisor installation is unavailable" >&2
  exit 1
}

available_kib="$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)"
[[ "$available_kib" =~ ^[0-9]+$ && "$available_kib" -ge 8388608 ]] || {
  echo "at least 8 GiB of available memory is required for the disposable matrix" >&2
  exit 1
}

if [[ -e "$OUTPUT" ]] && [[ -n "$(find "$OUTPUT" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]]; then
  echo "output directory must be absent or empty: $OUTPUT" >&2
  exit 1
fi
mkdir -p "$OUTPUT"

run_id="$(date -u +%Y%m%d%H%M%S)-$$"
docker_name="runtime-bench-docker-$run_id"
qemu_name="runtime-bench-qemu-$run_id"
cloud_name="runtime-bench-cloud-$run_id"
run_root="$(mktemp -d /tmp/agentic-runtime-benchmark.XXXXXX)"
config="$OUTPUT/config.json"

cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  docker rm -f "$docker_name" >/dev/null 2>&1 || true
  AGENTIC_BACKEND=libvirt \
    "$ROOT_DIR/scripts/destroy-vm.sh" "$qemu_name" --force >/dev/null 2>&1 || true
  AGENTIC_BACKEND=cloud-hypervisor \
    "$ROOT_DIR/scripts/destroy-vm.sh" "$cloud_name" --force >/dev/null 2>&1 || true
  case "$run_root" in
    /tmp/agentic-runtime-benchmark.*) find "$run_root" -depth -delete 2>/dev/null || true ;;
  esac
  exit "$rc"
}
trap cleanup EXIT INT TERM

ssh-keygen -q -t ed25519 -N '' -f "$run_root/bootstrap-key"

jq -n \
  --arg schema "agentic-sandbox.runtime-benchmark.v1" \
  --argjson samples "$SAMPLES" \
  --argjson warmups "$WARMUPS" \
  --arg docker_image "$DOCKER_IMAGE" \
  --arg docker_name "$docker_name" \
  --arg qemu_name "$qemu_name" \
  --arg cloud_name "$cloud_name" \
  --arg base_image "$BASE_IMAGE" \
  --arg bootstrap_public_key "$run_root/bootstrap-key.pub" \
  '{
    schema:$schema,
    samples:$samples,
    warmups:$warmups,
    timeout_seconds:900,
    workload:{cpu_iterations:250000,io_mib:64,task_iterations:10000},
    runtimes:[
      {
        name:"host",
        command:["python3","-"]
      },
      {
        name:"docker",
        prepare_command:["docker","run","-d","--name",$docker_name,"--network","none","--cpus","2","--memory","2g","--entrypoint","sleep",$docker_image,"infinity"],
        command:["docker","exec","-i",$docker_name,"python3","-"],
        cleanup_command:["docker","rm","-f",$docker_name]
      },
      {
        name:"qemu-libvirt",
        prepare_command:["env","AGENTIC_BACKEND=libvirt","AGENTIC_GRPC_VSOCK_PORT=8120","images/qemu/provision-vm.sh","--ssh-key",$bootstrap_public_key,"--base",$base_image,"--cpus","2","--memory","2G","--disk","48G","--start","--wait",$qemu_name],
        command:["scripts/runtime-benchmark-ssh-adapter.sh",$qemu_name],
        cleanup_command:["env","AGENTIC_BACKEND=libvirt","scripts/destroy-vm.sh",$qemu_name,"--force"]
      },
      {
        name:"cloud-hypervisor",
        prepare_command:["env","AGENTIC_BACKEND=cloud-hypervisor","AGENTIC_GRPC_VSOCK_PORT=8120","images/qemu/provision-vm.sh","--ssh-key",$bootstrap_public_key,"--base",$base_image,"--cpus","2","--memory","2G","--disk","48G","--start","--wait",$cloud_name],
        command:["scripts/runtime-benchmark-ssh-adapter.sh",$cloud_name],
        cleanup_command:["env","AGENTIC_BACKEND=cloud-hypervisor","scripts/destroy-vm.sh",$cloud_name,"--force"]
      }
    ]
  }' > "$config"

python3 "$ROOT_DIR/scripts/benchmark-runtimes.py" \
  --config "$config" \
  --output "$OUTPUT"

echo "complete runtime benchmark written to $OUTPUT"

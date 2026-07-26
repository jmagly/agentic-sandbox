#!/usr/bin/env bash
# Sanitized security-baseline verification. Prints only boolean/control-state
# evidence; never environment contents, tokens, command transcripts, or host
# inventory.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runtime="$repo_root/management/src/docker_runtime.rs"
agent="$repo_root/agent-rs/src/main.rs"
base="$repo_root/images/container/Dockerfile.base"

require_source() {
    local pattern="$1" file="$2" message="$3"
    if ! grep -q -- "$pattern" "$file"; then
        echo "FAIL: $message" >&2
        exit 1
    fi
}

require_source '"--user"' "$runtime" "managed Docker user is not enforced"
require_source '"--cap-drop"' "$runtime" "capability drop is not enforced"
require_source 'no-new-privileges:true' "$runtime" "no-new-privileges is not enforced"
require_source 'AGENT_BOOTSTRAP_INPUT_FILE' "$runtime" "tmpfs bootstrap handoff is absent"
require_source 'read_and_remove_bootstrap_token' "$agent" "agent-side token consumption/unlink is absent"
require_source '"network",' "$runtime" "Docker network lifecycle is absent"
require_source '"create",' "$runtime" "per-sandbox network creation is absent"
require_source 'useradd --uid 10001' "$base" "dedicated runtime identity is absent"

if [[ -n "${AGENTIC_SECURITY_IMAGE:-}" ]]; then
    image="$AGENTIC_SECURITY_IMAGE"
    output="$(
        docker run --rm \
            --user 10001:10001 \
            --cap-drop ALL \
            --security-opt no-new-privileges:true \
            --entrypoint sh \
            "$image" -c '
                uid="$(id -u)"
                cap_eff="$(sed -n "s/^CapEff:[[:space:]]*//p" /proc/self/status)"
                nnp="$(sed -n "s/^NoNewPrivs:[[:space:]]*//p" /proc/self/status)"
                forbidden=0
                for tool in grpcurl curl wget python python3; do
                    command -v "$tool" >/dev/null 2>&1 && forbidden=1
                done
                runtime_dirs=0
                [ -d /home/agent ] && [ -w /home/agent ] \
                    && [ -d /workspace ] && [ -w /workspace ] && runtime_dirs=1
                printf "uid_nonzero=%s capeff_zero=%s nnp_one=%s minimal_tools=%s runtime_dirs=%s\n" \
                    "$([ "$uid" -ne 0 ] && echo yes || echo no)" \
                    "$([ "$cap_eff" = 0000000000000000 ] && echo yes || echo no)" \
                    "$([ "$nnp" = 1 ] && echo yes || echo no)" \
                    "$([ "$forbidden" = 0 ] && echo yes || echo no)" \
                    "$([ "$runtime_dirs" = 1 ] && echo yes || echo no)"
            '
    )"
    expected="uid_nonzero=yes capeff_zero=yes nnp_one=yes minimal_tools=yes runtime_dirs=yes"
    [[ "$output" == "$expected" ]] || {
        echo "FAIL: live image security controls did not match baseline: $output" >&2
        exit 1
    }
    echo "PASS: live image security controls match baseline"

    probe_id="$$"
    target_network="agentic-security-target-$probe_id"
    source_network="agentic-security-source-$probe_id"
    target_container="agentic-security-target-$probe_id"
    cleanup_network_probe() {
        docker rm -f "$target_container" >/dev/null 2>&1 || true
        docker network rm "$target_network" >/dev/null 2>&1 || true
        docker network rm "$source_network" >/dev/null 2>&1 || true
    }
    trap cleanup_network_probe EXIT INT TERM
    docker network create --label agentic-security-verification=true "$target_network" >/dev/null
    docker network create --label agentic-security-verification=true "$source_network" >/dev/null
    docker run -d \
        --name "$target_container" \
        --network "$target_network" \
        --entrypoint openssl \
        "$image" s_server -accept 18443 -nocert -quiet -ign_eof >/dev/null
    target_ip="$(
        docker inspect \
            -f "{{(index .NetworkSettings.Networks \"$target_network\").IPAddress}}" \
            "$target_container"
    )"
    docker run --rm \
        --network "$target_network" \
        --entrypoint bash \
        -e TARGET_IP="$target_ip" \
        "$image" \
        -c 'timeout 5 bash -c "exec 3<>/dev/tcp/$TARGET_IP/18443"' >/dev/null
    if docker run --rm \
        --network "$source_network" \
        --entrypoint bash \
        -e TARGET_IP="$target_ip" \
        "$image" \
        -c 'timeout 2 bash -c "exec 3<>/dev/tcp/$TARGET_IP/18443"' \
        >/dev/null 2>&1; then
        echo "FAIL: cross-sandbox TCP reached a container on a distinct managed network" >&2
        exit 1
    fi
    cleanup_network_probe
    trap - EXIT INT TERM
    echo "PASS: live per-sandbox networks deny cross-network TCP"
else
    echo "PASS: managed-container security contracts are present"
fi

if [[ "${AGENTIC_REQUIRE_DEDICATED_AGENTSHARE:-0}" == 1 ]]; then
    agentshare="${AGENTSHARE_ROOT:-/srv/agentshare}"
    root_device="$(stat -c %d /)"
    share_device="$(stat -c %d "$agentshare")"
    [[ "$root_device" != "$share_device" ]] || {
        echo "FAIL: agentshare is on the host-root device" >&2
        exit 1
    }
    echo "PASS: agentshare uses a device distinct from host root"
fi

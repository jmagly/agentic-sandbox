#!/bin/bash
# Agentic Sandbox Launcher
# Launch isolated agent environments with Docker or QEMU
#
# Security features:
# - PID limits (fork bomb defense)
# - Memory limits (OOM protection)
# - Capability dropping (privilege reduction)
# - Seccomp syscall filtering
# - Network isolation with optional gateway access
# - Read-only root filesystem

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Defaults
RUNTIME="docker"
IMAGE="agent-claude"
MEMORY="8G"
CPUS="4"
PIDS_LIMIT="1024"
GPU=""
MOUNTS=()
ENV_VARS=()
AGENT_TASK=""
DETACH=false
NAME="sandbox-$(date +%s)"
NETWORK="isolated"
GATEWAY_URL=""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[sandbox]${NC} $1"; }
warn() { echo -e "${YELLOW}[sandbox]${NC} $1"; }
error() { echo -e "${RED}[sandbox]${NC} $1" >&2; }

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Launch an isolated agent sandbox environment.

Options:
    --runtime <docker|qemu>   Runtime type (default: docker)
    --image <name>            Image/VM name (default: agent-claude)
    --name <name>             Container/VM name
    --memory <size>           Memory limit (default: 8G)
    --cpus <count>            CPU count (default: 4)
    --pids-limit <count>      Max processes (default: 1024)
    --gpu <passthrough>       GPU passthrough mode (QEMU only)
    --mount <src:dst>         Mount directory into sandbox (repeatable)
    --env <KEY=value>         Set environment variable (repeatable)
    --network <mode>          Network: isolated, gateway, host (default: isolated)
    --gateway <url>           Gateway URL for MCP/API access
    --task <description>      Task for agent to complete
    --detach                  Run in background
    -h, --help                Show this help

Network Modes:
    isolated    No network access (most secure)
    gateway     Access via auth-injecting gateway only
    host        Full network access (not recommended)

Examples:
    # Launch with isolated network (default, most secure)
    $(basename "$0") --runtime docker --image sandbox-test

    # Launch with gateway access to MCP servers
    $(basename "$0") --runtime docker --image sandbox-test \\
        --network gateway --gateway http://gateway:8080 \\
        --mount ./workspace:/workspace

    # Launch autonomous task
    $(basename "$0") --task "Refactor the authentication module" --detach

    # Launch QEMU VM with GPU
    $(basename "$0") --runtime qemu --gpu passthrough --memory 16G
EOF
    exit 0
}

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --runtime) RUNTIME="$2"; shift 2 ;;
        --image) IMAGE="$2"; shift 2 ;;
        --name) NAME="$2"; shift 2 ;;
        --memory) MEMORY="$2"; shift 2 ;;
        --cpus) CPUS="$2"; shift 2 ;;
        --pids-limit) PIDS_LIMIT="$2"; shift 2 ;;
        --gpu) GPU="$2"; shift 2 ;;
        --mount) MOUNTS+=("$2"); shift 2 ;;
        --env) ENV_VARS+=("$2"); shift 2 ;;
        --network) NETWORK="$2"; shift 2 ;;
        --gateway) GATEWAY_URL="$2"; shift 2 ;;
        --task) AGENT_TASK="$2"; shift 2 ;;
        --detach) DETACH=true; shift ;;
        -h|--help) usage ;;
        *) error "Unknown option: $1"; usage ;;
    esac
done

launch_docker() {
    log "Launching Docker sandbox: $NAME"

    # Build docker run arguments
    local -a args=("run")

    # Container name and hostname
    args+=("--name" "$NAME")
    args+=("--hostname" "sandbox")

    # Resource limits
    args+=("--memory=$MEMORY")
    args+=("--cpus=$CPUS")
    args+=("--pids-limit=$PIDS_LIMIT")

    # Security hardening
    args+=("--cap-drop" "ALL")
    args+=("--security-opt" "no-new-privileges:true")

    # Labels for management server discovery/cleanup
    args+=("--label" "agentic-sandbox=true")
    args+=("--label" "agentic-runtime=docker")

    # Seccomp profile (if exists)
    local seccomp_profile="$PROJECT_ROOT/configs/seccomp-agent.json"
    if [[ -f "$seccomp_profile" ]]; then
        args+=("--security-opt" "seccomp=$seccomp_profile")
        log "Using seccomp profile: $seccomp_profile"
    fi

    # Read-only root filesystem with writable /tmp
    args+=("--read-only")
    args+=("--tmpfs" "/tmp:noexec,nosuid,size=1g")
    args+=("--tmpfs" "/var/tmp:noexec,nosuid,size=256m")

    # Network configuration
    case "$NETWORK" in
        isolated)
            args+=("--network" "none")
            log "Network: isolated (no external access)"
            ;;
        gateway)
            # Create internal network if needed
            if ! docker network inspect sandbox-net >/dev/null 2>&1; then
                log "Creating sandbox-net bridge network..."
                docker network create --internal sandbox-net
            fi
            args+=("--network" "sandbox-net")
            if [[ -n "$GATEWAY_URL" ]]; then
                args+=("-e" "HTTP_PROXY=$GATEWAY_URL")
                args+=("-e" "HTTPS_PROXY=$GATEWAY_URL")
                args+=("-e" "GATEWAY_URL=$GATEWAY_URL")
            fi
            log "Network: gateway (access via $GATEWAY_URL)"
            ;;
        host)
            warn "Using host network - NOT RECOMMENDED for production"
            args+=("--network" "host")
            ;;
        *)
            error "Unknown network mode: $NETWORK"
            exit 1
            ;;
    esac

    # Volume mounts
    for mount in "${MOUNTS[@]:-}"; do
        if [[ -n "$mount" ]]; then
            args+=("-v" "$mount")
            log "Mount: $mount"
        fi
    done

    # Environment variables
    for env_var in "${ENV_VARS[@]:-}"; do
        if [[ -n "$env_var" ]]; then
            args+=("-e" "$env_var")
        fi
    done

    # Agent task
    if [[ -n "$AGENT_TASK" ]]; then
        args+=("-e" "AGENT_TASK=$AGENT_TASK")
        args+=("-e" "AGENT_MODE=autonomous")
    fi

    # Detach or interactive
    if [[ "$DETACH" == "true" ]]; then
        args+=("-d")
    else
        args+=("-it" "--rm")
    fi

    # Image
    local full_image="$IMAGE"
    if [[ "$IMAGE" != *":"* ]] && [[ "$IMAGE" != *"/"* ]]; then
        full_image="agentic-sandbox-$IMAGE:latest"
    fi
    args+=("$full_image")

    # Command (if task specified)
    if [[ -n "$AGENT_TASK" ]]; then
        args+=("bash" "-c" "echo 'Task: $AGENT_TASK' && exec bash")
    fi

    log "Executing: docker ${args[*]}"
    docker "${args[@]}"
}

launch_qemu() {
    log "Launching QEMU VM: $NAME"

    local vm_config="$PROJECT_ROOT/runtimes/qemu/$IMAGE.xml"

    if [[ ! -f "$vm_config" ]]; then
        error "VM configuration not found: $vm_config"
        exit 1
    fi

    # Check if VM already exists
    if virsh dominfo "$NAME" >/dev/null 2>&1; then
        log "VM $NAME already exists"
        local state
        state=$(virsh domstate "$NAME" 2>/dev/null || echo "unknown")
        if [[ "$state" == "running" ]]; then
            log "VM is already running"
        else
            log "Starting VM..."
            virsh start "$NAME"
        fi
    else
        # Create VM from template
        local tmp_config="/tmp/$NAME.xml"
        sed "s/<name>.*<\/name>/<name>$NAME<\/name>/" "$vm_config" > "$tmp_config"

        # Adjust memory
        local mem_gb="${MEMORY%G}"
        sed -i "s/<memory unit='GiB'>.*<\/memory>/<memory unit='GiB'>$mem_gb<\/memory>/" "$tmp_config"
        sed -i "s/<currentMemory unit='GiB'>.*<\/currentMemory>/<currentMemory unit='GiB'>$mem_gb<\/currentMemory>/" "$tmp_config"

        # Adjust CPUs
        sed -i "s/<vcpu placement='static'>.*<\/vcpu>/<vcpu placement='static'>$CPUS<\/vcpu>/" "$tmp_config"

        # TODO: Handle GPU passthrough if specified

        log "Defining VM from $vm_config"
        virsh define "$tmp_config"
        virsh start "$NAME"
        rm -f "$tmp_config"
    fi

    # Connect to console or run in background
    if [[ "$DETACH" != "true" ]]; then
        log "Connecting to console (Ctrl+] to exit)..."
        sleep 2
        virsh console "$NAME"
    else
        log "VM running in background"
        log "Connect with: virsh console $NAME"
        log "Stop with: virsh destroy $NAME"
    fi
}

# Main
case "$RUNTIME" in
    docker)
        launch_docker
        ;;
    qemu)
        launch_qemu
        ;;
    *)
        error "Unknown runtime: $RUNTIME"
        exit 1
        ;;
esac

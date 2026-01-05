#!/bin/bash
# Agentic Sandbox Launcher
# Launch isolated agent environments with Docker or QEMU

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Defaults
RUNTIME="docker"
IMAGE="agent-claude"
MEMORY="8G"
CPUS="4"
GPU=""
MOUNT=""
ENV_VARS=()
AGENT_TASK=""
DETACH=false
NAME="sandbox-$(date +%s)"

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
    --gpu <passthrough>       GPU passthrough mode (QEMU only)
    --mount <src:dst>         Mount directory into sandbox
    --env <KEY=value>         Set environment variable
    --task <description>      Task for agent to complete
    --detach                  Run in background
    -h, --help                Show this help

Examples:
    # Launch interactive Claude agent
    $(basename "$0") --runtime docker --image agent-claude

    # Launch with mounted workspace
    $(basename "$0") --mount ./project:/workspace/project

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
        --runtime)
            RUNTIME="$2"
            shift 2
            ;;
        --image)
            IMAGE="$2"
            shift 2
            ;;
        --name)
            NAME="$2"
            shift 2
            ;;
        --memory)
            MEMORY="$2"
            shift 2
            ;;
        --cpus)
            CPUS="$2"
            shift 2
            ;;
        --gpu)
            GPU="$2"
            shift 2
            ;;
        --mount)
            MOUNT="$2"
            shift 2
            ;;
        --env)
            ENV_VARS+=("$2")
            shift 2
            ;;
        --task)
            AGENT_TASK="$2"
            shift 2
            ;;
        --detach)
            DETACH=true
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            echo "Unknown option: $1"
            usage
            ;;
    esac
done

launch_docker() {
    echo "[sandbox] Launching Docker sandbox: $NAME"

    # Build command
    local cmd="docker run"

    if [ "$DETACH" = true ]; then
        cmd+=" -d"
    else
        cmd+=" -it --rm"
    fi

    cmd+=" --name $NAME"
    cmd+=" --hostname sandbox"
    cmd+=" --memory=$MEMORY"
    cmd+=" --cpus=$CPUS"
    cmd+=" --security-opt no-new-privileges:true"
    cmd+=" --cap-drop ALL"
    cmd+=" --cap-add NET_BIND_SERVICE"
    cmd+=" --cap-add CHOWN"
    cmd+=" --cap-add SETUID"
    cmd+=" --cap-add SETGID"

    # Mount workspace
    if [ -n "$MOUNT" ]; then
        cmd+=" -v $MOUNT"
    fi

    # Environment variables
    for env_var in "${ENV_VARS[@]:-}"; do
        cmd+=" -e $env_var"
    done

    if [ -n "$AGENT_TASK" ]; then
        cmd+=" -e AGENT_TASK='$AGENT_TASK'"
        cmd+=" -e AGENT_MODE=autonomous"
    fi

    # API key from environment
    if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
        cmd+=" -e ANTHROPIC_API_KEY"
    fi

    cmd+=" agentic-sandbox-$IMAGE:latest"

    echo "[sandbox] Executing: $cmd"
    eval "$cmd"
}

launch_qemu() {
    echo "[sandbox] Launching QEMU VM: $NAME"

    local vm_config="$PROJECT_ROOT/runtimes/qemu/$IMAGE.xml"

    if [ ! -f "$vm_config" ]; then
        echo "Error: VM configuration not found: $vm_config"
        exit 1
    fi

    # Create VM from template
    local tmp_config="/tmp/$NAME.xml"
    sed "s/<name>.*<\/name>/<name>$NAME<\/name>/" "$vm_config" > "$tmp_config"

    # Adjust memory
    local mem_gb="${MEMORY%G}"
    sed -i "s/<memory unit='GiB'>.*<\/memory>/<memory unit='GiB'>$mem_gb<\/memory>/" "$tmp_config"
    sed -i "s/<currentMemory unit='GiB'>.*<\/currentMemory>/<currentMemory unit='GiB'>$mem_gb<\/currentMemory>/" "$tmp_config"

    # Adjust CPUs
    sed -i "s/<vcpu placement='static'>.*<\/vcpu>/<vcpu placement='static'>$CPUS<\/vcpu>/" "$tmp_config"

    # Define and start VM
    virsh define "$tmp_config"

    if [ "$DETACH" = true ]; then
        virsh start "$NAME"
        echo "[sandbox] VM started in background: $NAME"
        echo "[sandbox] Connect with: virsh console $NAME"
    else
        virsh start "$NAME"
        virsh console "$NAME"
    fi

    rm -f "$tmp_config"
}

# Main
case $RUNTIME in
    docker)
        launch_docker
        ;;
    qemu)
        launch_qemu
        ;;
    *)
        echo "Error: Unknown runtime: $RUNTIME"
        exit 1
        ;;
esac

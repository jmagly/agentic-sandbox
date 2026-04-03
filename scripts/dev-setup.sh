#!/bin/bash
# Development environment setup script
# Sets up dependencies, builds binaries, and creates development network

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

log_info() {
    echo -e "${GREEN}[INFO]${NC} $*"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $*"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

log_step() {
    echo -e "\n${BLUE}==>${NC} $*"
}

check_rust() {
    log_step "Checking Rust toolchain"

    if ! command -v cargo &>/dev/null; then
        log_error "Rust (cargo) is not installed"
        log_info "Install Rust from: https://rustup.rs/"
        return 1
    fi

    log_info "Found Rust: $(rustc --version)"

    # Ensure musl target is installed (needed for Alpine VM agent builds)
    if ! rustup target list --installed | grep -q x86_64-unknown-linux-musl; then
        log_info "Adding musl target for Alpine builds..."
        rustup target add x86_64-unknown-linux-musl
    else
        log_info "musl target already installed"
    fi

    return 0
}

check_python() {
    log_step "Checking Python"

    if ! command -v python &>/dev/null; then
        log_warn "Python is not installed; Python SDK tests will be skipped"
        return 0
    fi

    log_info "Found Python: $(python --version)"
    return 0
}

check_docker() {
    log_step "Checking Docker"

    if ! command -v docker &>/dev/null; then
        log_error "Docker is not installed"
        log_info "Install Docker from: https://docs.docker.com/get-docker/"
        return 1
    fi

    if ! docker info &>/dev/null; then
        log_error "Docker daemon is not running or not accessible"
        log_info "Make sure Docker is running and your user is in the docker group"
        log_info "Run: sudo usermod -aG docker \$USER && newgrp docker"
        return 1
    fi

    log_info "Docker is available"
    return 0
}

build_binaries() {
    log_step "Building Rust components"

    cd "$PROJECT_ROOT"
    make build

    log_info "Binaries built successfully"
}

build_docker_images() {
    log_step "Building Docker images"

    cd "$PROJECT_ROOT"

    log_info "Building base image..."
    make docker-base

    log_info "Building test image..."
    make docker-test

    log_info "Docker images built successfully"
    if command -v rg &>/dev/null; then
        docker images | rg agentic-sandbox || true
    else
        docker images | grep agentic-sandbox || true
    fi
}

create_development_network() {
    log_step "Creating development network"

    local network_name="sandbox-dev-network"

    if docker network inspect "$network_name" &>/dev/null; then
        log_info "Network $network_name already exists"
    else
        log_info "Creating network $network_name..."
        docker network create \
            --driver bridge \
            "$network_name"
        log_info "Network $network_name created"
    fi
}

show_usage() {
    cat << 'USAGEEOF'
Usage: scripts/dev-setup.sh [options]

Options:
  --skip-docker    Skip Docker checks and image builds
  --no-build       Skip building Rust binaries
  --help           Show this help message
USAGEEOF
}

main() {
    local skip_docker=false
    local skip_build=false

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --skip-docker)
                skip_docker=true
                shift
                ;;
            --no-build)
                skip_build=true
                shift
                ;;
            --help|-h)
                show_usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                show_usage
                exit 1
                ;;
        esac
    done

    check_rust || exit 1
    check_python || true

    if [ "$skip_docker" = false ]; then
        check_docker || exit 1
    fi

    if [ "$skip_build" = false ]; then
        build_binaries
    fi

    if [ "$skip_docker" = false ]; then
        build_docker_images
        create_development_network
    fi

    log_info "Development environment setup complete!"
}

main "$@"

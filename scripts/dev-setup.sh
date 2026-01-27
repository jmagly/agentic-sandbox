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

check_go_version() {
    log_step "Checking Go version"

    if ! command -v go &>/dev/null; then
        log_error "Go is not installed"
        log_info "Install Go from: https://go.dev/dl/"
        return 1
    fi

    local go_version
    go_version=$(go version | awk '{print $3}' | sed 's/go//')
    local required_version="1.22"

    log_info "Found Go version: $go_version"

    # Simple version check (comparing major.minor)
    local current_major_minor
    current_major_minor=$(echo "$go_version" | cut -d. -f1,2)

    if [ "$(printf '%s\n' "$required_version" "$current_major_minor" | sort -V | head -n1)" != "$required_version" ]; then
        log_error "Go version $required_version or higher is required"
        return 1
    fi

    log_info "Go version is compatible"
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

install_go_dependencies() {
    log_step "Installing Go dependencies"

    cd "$PROJECT_ROOT"

    if [ ! -f "go.mod" ]; then
        log_warn "go.mod not found, initializing Go module"
        go mod init github.com/roctinam/agentic-sandbox
    fi

    log_info "Downloading dependencies..."
    go mod download

    log_info "Verifying dependencies..."
    go mod verify

    log_info "Tidying go.mod..."
    go mod tidy

    log_info "Go dependencies installed"
}

install_development_tools() {
    log_step "Installing development tools"

    log_info "Installing golangci-lint..."
    if ! command -v golangci-lint &>/dev/null; then
        curl -sSfL https://raw.githubusercontent.com/golangci/golangci-lint/master/install.sh | sh -s -- -b "$(go env GOPATH)/bin" v1.55.2
        log_info "golangci-lint installed"
    else
        log_info "golangci-lint already installed"
    fi

    log_info "Installing air (live reload)..."
    if ! command -v air &>/dev/null; then
        go install github.com/cosmtrek/air@latest
        log_info "air installed"
    else
        log_info "air already installed"
    fi

    log_info "Installing delve (debugger)..."
    if ! command -v dlv &>/dev/null; then
        go install github.com/go-delve/delve/cmd/dlv@latest
        log_info "delve installed"
    else
        log_info "delve already installed"
    fi

    # Add GOPATH/bin to PATH if not already there
    local gopath_bin
    gopath_bin="$(go env GOPATH)/bin"
    if [[ ":$PATH:" != *":$gopath_bin:"* ]]; then
        log_warn "Add $gopath_bin to your PATH"
        log_info "Add this to your ~/.bashrc or ~/.zshrc:"
        echo "    export PATH=\"\$PATH:$gopath_bin\""
    fi
}

build_binaries() {
    log_step "Building binaries"

    cd "$PROJECT_ROOT"

    log_info "Building sandbox-manager..."
    make build-manager

    log_info "Building sandbox-cli..."
    make build-cli

    log_info "Binaries built successfully"
    ls -lh "$PROJECT_ROOT/bin/"
}

build_docker_images() {
    log_step "Building Docker images"

    cd "$PROJECT_ROOT"

    log_info "Building base image..."
    make docker-base

    log_info "Building test image..."
    make docker-test

    log_info "Docker images built successfully"
    docker images | grep agentic-sandbox
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
            --subnet 172.30.0.0/16 \
            --opt com.docker.network.bridge.name=br-sandbox-dev \
            "$network_name"
        log_info "Network created"
    fi
}

setup_git_hooks() {
    log_step "Setting up Git hooks"

    cd "$PROJECT_ROOT"

    if [ ! -d ".git" ]; then
        log_warn "Not a Git repository, skipping Git hooks"
        return 0
    fi

    local hooks_dir=".git/hooks"
    mkdir -p "$hooks_dir"

    # Pre-commit hook
    cat > "$hooks_dir/pre-commit" << 'EOF'
#!/bin/bash
# Pre-commit hook: format and lint

echo "Running pre-commit checks..."

# Format code
make fmt

# Run linter
if ! make lint; then
    echo "Linting failed. Please fix errors before committing."
    exit 1
fi

# Run tests
if ! make test; then
    echo "Tests failed. Please fix before committing."
    exit 1
fi

echo "Pre-commit checks passed"
EOF

    chmod +x "$hooks_dir/pre-commit"
    log_info "Git hooks installed"
}

create_config_files() {
    log_step "Creating configuration files"

    cd "$PROJECT_ROOT"

    # Create configs directory if it doesn't exist
    mkdir -p configs

    # Create sample manager config
    if [ ! -f "configs/manager.yaml" ]; then
        cat > "configs/manager.yaml" << 'EOF'
# Sandbox Manager Configuration
api:
  addr: ":8080"
  log_level: debug

runtime:
  docker:
    host: unix:///var/run/docker.sock
    network: sandbox-dev-network
  qemu:
    socket_path: /var/run/libvirt/libvirt-sock

storage:
  data_dir: /var/lib/sandbox/data
  max_size: 10G

security:
  seccomp_profile: /etc/sandbox/configs/seccomp-default.json
  default_capabilities:
    - NET_ADMIN
EOF
        log_info "Created configs/manager.yaml"
    else
        log_info "configs/manager.yaml already exists"
    fi

    # Create .air.toml for live reload
    if [ ! -f ".air.toml" ]; then
        cat > ".air.toml" << 'EOF'
root = "."
testdata_dir = "testdata"
tmp_dir = "tmp"

[build]
  args_bin = []
  bin = "./tmp/main"
  cmd = "go build -o ./tmp/main ./cmd/sandbox-manager"
  delay = 1000
  exclude_dir = ["assets", "tmp", "vendor", "testdata"]
  exclude_file = []
  exclude_regex = ["_test.go"]
  exclude_unchanged = false
  follow_symlink = false
  full_bin = ""
  include_dir = []
  include_ext = ["go", "tpl", "tmpl", "html"]
  include_file = []
  kill_delay = "0s"
  log = "build-errors.log"
  poll = false
  poll_interval = 0
  rerun = false
  rerun_delay = 500
  send_interrupt = false
  stop_on_error = false

[color]
  app = ""
  build = "yellow"
  main = "magenta"
  runner = "green"
  watcher = "cyan"

[log]
  main_only = false
  time = false

[misc]
  clean_on_exit = false

[screen]
  clear_on_rebuild = false
  keep_scroll = true
EOF
        log_info "Created .air.toml for live reload"
    else
        log_info ".air.toml already exists"
    fi
}

print_next_steps() {
    log_step "Setup complete!"

    cat << EOF

${GREEN}Next steps:${NC}

1. Start development environment:
   ${BLUE}make dev-up${NC}

2. View logs:
   ${BLUE}make dev-logs${NC}

3. Run tests:
   ${BLUE}make test${NC}

4. Run integration tests:
   ${BLUE}make integration-test${NC}

5. Live reload during development:
   ${BLUE}air${NC}

6. Stop development environment:
   ${BLUE}make dev-down${NC}

${YELLOW}Development tools installed:${NC}
  - golangci-lint (linter)
  - air (live reload)
  - delve (debugger)

${YELLOW}Useful commands:${NC}
  make help       - Show all available commands
  make check      - Run all checks before commit
  make build      - Build binaries
  make docker     - Build Docker images

EOF
}

main() {
    log_info "Starting development environment setup"
    log_info "Project root: $PROJECT_ROOT"

    check_go_version || exit 1
    check_docker || exit 1
    install_go_dependencies
    install_development_tools
    create_config_files
    build_binaries
    build_docker_images
    create_development_network
    setup_git_hooks

    print_next_steps
}

main "$@"

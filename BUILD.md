# Build Infrastructure

Complete build, test, and CI/CD documentation for agentic-sandbox.

## Quick Start

```bash
# Initial setup
./scripts/dev-setup.sh

# Build everything
make build

# Run tests
make test

# Start development environment
make dev-up
```

## Prerequisites

- **Go 1.22+** - [Install Go](https://go.dev/dl/)
- **Docker** - [Install Docker](https://docs.docker.com/get-docker/)
- **Make** - Build automation
- **git** - Version control

## Development Workflow

### Initial Setup

Run the development setup script to install dependencies and configure your environment:

```bash
./scripts/dev-setup.sh
```

This script will:
- Check Go and Docker versions
- Install Go dependencies
- Install development tools (golangci-lint, air, delve)
- Build binaries
- Build Docker images
- Create development network
- Set up Git hooks

### Building

```bash
# Build all binaries
make build

# Build individual components
make build-manager  # Build sandbox-manager
make build-cli      # Build sandbox-cli

# Build Docker images
make docker         # Build all images
make docker-base    # Build base image only
make docker-test    # Build test image only
```

### Testing

```bash
# Run unit tests
make test

# Run tests with coverage
make test-coverage

# Run integration tests
make integration-test

# Run all checks (format, lint, test)
make check
```

### Code Quality

```bash
# Format code
make fmt

# Run linter
make lint

# Run go vet
make vet

# Run all checks before commit
make check
```

### Development Environment

Start the complete development environment with Docker Compose:

```bash
# Start all services
make dev-up

# View logs
make dev-logs

# Stop services
make dev-down
```

Services included:
- **sandbox-manager** - Main API server (port 8080)
- **gateway** - Nginx reverse proxy (port 80)
- **test-sandbox** - Test sandbox instance (on demand)

### Live Reload

Use [air](https://github.com/cosmtrek/air) for automatic rebuild during development:

```bash
# Start with live reload
air

# air will watch for changes and rebuild automatically
```

Configuration is in `.air.toml`.

### Debugging

Use [delve](https://github.com/go-delve/delve) for debugging:

```bash
# Debug sandbox-manager
dlv debug ./cmd/sandbox-manager

# Debug with arguments
dlv debug ./cmd/sandbox-manager -- --config configs/manager.yaml

# Debug tests
dlv test ./internal/runtime
```

## Make Targets

Run `make help` to see all available targets:

| Target | Description |
|--------|-------------|
| `build` | Build all binaries |
| `build-manager` | Build sandbox-manager |
| `build-cli` | Build sandbox-cli |
| `test` | Run unit tests |
| `test-coverage` | Run tests with coverage report |
| `lint` | Run golangci-lint |
| `fmt` | Format Go code |
| `vet` | Run go vet |
| `check` | Run all checks (fmt, vet, lint, test) |
| `docker` | Build all Docker images |
| `docker-base` | Build base image |
| `docker-test` | Build test image |
| `integration-test` | Run integration tests |
| `dev-setup` | Set up development environment |
| `dev-up` | Start development environment |
| `dev-down` | Stop development environment |
| `dev-logs` | View development logs |
| `clean` | Remove build artifacts |
| `clean-all` | Remove all artifacts and images |
| `install` | Install binaries to system |
| `help` | Show all targets |

## CI/CD Pipeline

### Gitea Actions

CI pipeline is defined in `.gitea/workflows/ci.yaml`. It runs on:
- Push to `main` or `develop` branches
- Pull requests to `main` or `develop`

Pipeline stages:

1. **Lint** - Code formatting and static analysis
2. **Test** - Unit tests with coverage
3. **Build** - Compile binaries
4. **Docker** - Build and test images
5. **Integration** - Integration tests
6. **Security** - Trivy vulnerability scanning

### Manual Workflow

```bash
# What CI does locally
make check           # Lint and test
make build           # Build binaries
make docker          # Build images
make integration-test # Integration tests
```

## Docker Images

### Base Image

Minimal Ubuntu-based image with common tools:

```bash
# Build
make docker-base

# Run interactively
docker run -it agentic-sandbox-base:latest /bin/bash

# Test
docker run --rm agentic-sandbox-base:latest id agent
```

Image includes:
- Ubuntu 22.04
- Non-root user `agent` (UID 1000)
- Essential tools: curl, wget, git, ca-certificates
- Tini init system

### Test Image

Extended base image for testing:

```bash
# Build
make docker-test

# Run tests
docker run --rm agentic-sandbox-test:latest /bin/bash -c "echo 'Test passed'"
```

## Integration Testing

Integration tests validate the complete system:

```bash
# Run integration tests
make integration-test

# Or directly
./scripts/integration-test.sh
```

Tests cover:
- Binary existence and functionality
- Docker image builds
- Sandbox manager startup
- Sandbox creation and lifecycle
- Command execution
- Security isolation
- Cleanup procedures

## Development Tools

### golangci-lint

Comprehensive Go linter configured in `.golangci.yml`:

```bash
# Run linter
make lint

# Or directly
golangci-lint run
```

Enabled linters:
- errcheck, gosimple, govet, staticcheck
- gofmt, goimports, misspell
- gosec (security), goconst, gocyclo

### Air (Live Reload)

Automatically rebuilds on file changes:

```bash
# Start with live reload
air

# Configuration in .air.toml
```

### Delve (Debugger)

Debug Go applications:

```bash
# Debug manager
dlv debug ./cmd/sandbox-manager

# Debug with breakpoints
dlv debug ./cmd/sandbox-manager
(dlv) break main.main
(dlv) continue
```

## Configuration Files

| File | Purpose |
|------|---------|
| `Makefile` | Build automation |
| `.gitea/workflows/ci.yaml` | CI/CD pipeline |
| `docker-compose.dev.yaml` | Development environment |
| `Dockerfile.dev` | Development container |
| `.golangci.yml` | Linter configuration |
| `.air.toml` | Live reload configuration |
| `.gitignore` | Git ignore patterns |
| `configs/nginx.conf` | Gateway configuration |

## Directory Structure

```
agentic-sandbox/
├── .gitea/workflows/     # CI/CD workflows
├── bin/                  # Compiled binaries (gitignored)
├── cmd/                  # Command entry points
│   ├── sandbox-manager/
│   └── sandbox-cli/
├── configs/              # Configuration files
├── images/               # Docker images
│   ├── base/            # Base image
│   └── test/            # Test image
├── internal/             # Internal packages
├── scripts/              # Build and test scripts
│   ├── dev-setup.sh
│   └── integration-test.sh
├── Makefile             # Build automation
├── docker-compose.dev.yaml
├── Dockerfile.dev
└── .golangci.yml
```

## Troubleshooting

### Go Module Issues

```bash
# Clean and rebuild
go clean -modcache
go mod download
go mod verify
```

### Docker Build Issues

```bash
# Clean Docker build cache
docker builder prune

# Rebuild without cache
docker build --no-cache -t agentic-sandbox-base:latest images/base/
```

### Permission Issues

```bash
# Add user to docker group
sudo usermod -aG docker $USER
newgrp docker

# Verify Docker access
docker ps
```

### Integration Test Failures

```bash
# Clean up test resources
docker ps -a --filter "name=sandbox-test-" -q | xargs -r docker rm -f
docker network ls --filter "name=sandbox-test-" -q | xargs -r docker network rm

# Rebuild everything
make clean-all
make build
make docker
make integration-test
```

## Best Practices

### Before Committing

Always run checks before committing:

```bash
make check
```

Or rely on Git pre-commit hook (installed by `dev-setup.sh`).

### Code Style

- Run `make fmt` to format code
- Follow [Go Code Review Comments](https://github.com/golang/go/wiki/CodeReviewComments)
- Keep functions small and focused
- Write tests for new functionality

### Commit Messages

Follow conventional commits:

```
type(scope): subject

body
```

Types: feat, fix, docs, test, refactor, chore

### Pull Requests

1. Create feature branch: `git checkout -b feature/your-feature`
2. Make changes and commit
3. Run `make check` to validate
4. Push and create PR
5. Wait for CI to pass

## Performance

### Build Performance

- Go module cache speeds up builds
- Docker layer caching reduces rebuild time
- Parallel make targets where possible

### Test Performance

- Unit tests: ~5 seconds
- Integration tests: ~30 seconds
- Full CI pipeline: ~3 minutes

### Resource Usage

Development environment:
- sandbox-manager: ~50MB RAM, 0.1 CPU
- gateway: ~10MB RAM, 0.05 CPU
- test-sandbox: ~100MB RAM, 0.2 CPU

## Security

### Build Security

- No credentials in CI/CD
- Secrets in environment variables
- Security scanning with Trivy
- Dependency vulnerability checks

### Container Security

- Non-root users
- Read-only filesystems where possible
- Capability dropping
- Seccomp profiles
- Network isolation

## References

- [Go Documentation](https://go.dev/doc/)
- [Docker Documentation](https://docs.docker.com/)
- [golangci-lint](https://golangci-lint.run/)
- [Gitea Actions](https://docs.gitea.io/en-us/actions/)

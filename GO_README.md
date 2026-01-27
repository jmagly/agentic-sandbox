# Agentic Sandbox Go Implementation

Go-based REST API server and CLI for managing isolated agent sandbox environments.

## Project Structure

```
cmd/
  sandbox-manager/        # REST API server
  sandbox-cli/            # CLI tool
internal/
  api/                    # HTTP server and handlers
  runtime/                # Runtime adapter interface and implementations
  sandbox/                # Sandbox domain model and lifecycle manager
  config/                 # Configuration loading
pkg/
  client/                 # Go client library
```

## Prerequisites

- Go 1.23 or later
- Docker (for Docker runtime)
- QEMU/libvirt (for QEMU runtime)

## Installation

```bash
# Install dependencies
go mod download

# Build server
go build -o bin/sandbox-manager ./cmd/sandbox-manager

# Build CLI
go build -o bin/sandbox-cli ./cmd/sandbox-cli
```

## Running Tests

```bash
# Run all tests
go test ./...

# Run tests with coverage
go test -cover ./...

# Run tests with verbose output
go test -v ./...

# Run specific package tests
go test ./internal/sandbox
go test ./internal/api
go test ./internal/runtime

# Generate coverage report
go test -coverprofile=coverage.out ./...
go tool cover -html=coverage.out -o coverage.html
```

## Test-First Development

This project follows strict test-first development:

1. All tests are written BEFORE implementation
2. Tests define expected behavior and API contracts
3. Implementation makes tests pass
4. Minimum 80% code coverage required

### Current Test Coverage

Run `make test-coverage` to see current coverage:

```bash
make test-coverage
```

Expected coverage by package:
- `internal/sandbox`: 80%+
- `internal/config`: 80%+
- `internal/api`: 80%+
- `internal/runtime`: (stubs - will increase with implementation)
- `pkg/client`: 80%+

## Running the Server

```bash
# Start server with default config
./bin/sandbox-manager

# Start with environment variables
SERVER_PORT=9090 \
SECURITY_ENABLE_SECCOMP=true \
DOCKER_SECCOMP_PROFILE=/path/to/seccomp.json \
./bin/sandbox-manager
```

Server will start on `http://0.0.0.0:8080` by default.

## Using the CLI

```bash
# Check server health
./bin/sandbox-cli health

# Create a sandbox
./bin/sandbox-cli create \
  --name my-agent \
  --runtime docker \
  --image agent-claude \
  --cpu 4 \
  --memory 8G \
  --pids-limit 1024 \
  --network isolated \
  --auto-start

# List sandboxes
./bin/sandbox-cli list

# Get sandbox details
./bin/sandbox-cli get <sandbox-id>

# Start sandbox
./bin/sandbox-cli start <sandbox-id>

# Stop sandbox
./bin/sandbox-cli stop <sandbox-id>

# Delete sandbox
./bin/sandbox-cli delete <sandbox-id>
```

## Using the Go Client Library

```go
package main

import (
	"context"
	"fmt"

	"github.com/roctinam/agentic-sandbox/internal/sandbox"
	"github.com/roctinam/agentic-sandbox/pkg/client"
)

func main() {
	c := client.NewClient("http://localhost:8080")
	ctx := context.Background()

	// Create sandbox
	spec := &sandbox.SandboxSpec{
		Name:    "my-agent",
		Runtime: "docker",
		Image:   "agent-claude:latest",
		Resources: sandbox.Resources{
			CPU:       "4",
			Memory:    "8G",
			PidsLimit: 1024,
		},
		Network:   sandbox.NetworkIsolated,
		AutoStart: true,
	}

	sb, err := c.CreateSandbox(ctx, spec)
	if err != nil {
		panic(err)
	}

	fmt.Printf("Created sandbox: %s\n", sb.ID)

	// List sandboxes
	sandboxes, err := c.ListSandboxes(ctx)
	if err != nil {
		panic(err)
	}

	for _, sb := range sandboxes {
		fmt.Printf("Sandbox: %s (state: %s)\n", sb.Name, sb.State)
	}
}
```

## API Endpoints

### Health Check
- `GET /health` - Server health check

### Sandboxes
- `GET /api/v1/sandboxes` - List all sandboxes
- `POST /api/v1/sandboxes` - Create new sandbox
- `GET /api/v1/sandboxes/{id}` - Get sandbox details
- `DELETE /api/v1/sandboxes/{id}` - Delete sandbox
- `POST /api/v1/sandboxes/{id}/start` - Start sandbox
- `POST /api/v1/sandboxes/{id}/stop` - Stop sandbox

### TODO Endpoints
- `POST /api/v1/sandboxes/{id}/exec` - Execute command
- `GET /api/v1/sandboxes/{id}/logs` - Get logs
- `GET /api/v1/sandboxes/{id}/stats` - Get resource stats

## Configuration

Configuration is loaded from environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SERVER_HOST` | `0.0.0.0` | Server bind address |
| `SERVER_PORT` | `8080` | Server port |
| `DOCKER_SECCOMP_PROFILE` | `/etc/agentic-sandbox/seccomp-agent.json` | Seccomp profile path |
| `DOCKER_DEFAULT_NETWORK` | `isolated` | Default network mode |
| `QEMU_LIBVIRT_URI` | `qemu:///system` | Libvirt connection URI |
| `QEMU_TEMPLATES_PATH` | `/etc/agentic-sandbox/qemu` | VM template directory |
| `SECURITY_ENABLE_SECCOMP` | `true` | Enable seccomp filtering |
| `SECURITY_ENABLE_APPARMOR` | `false` | Enable AppArmor profiles |
| `SECURITY_DEFAULT_PIDS_LIMIT` | `1024` | Default PID limit |
| `SECURITY_DEFAULT_MEMORY_MB` | `8192` | Default memory limit (MB) |
| `SECURITY_DEFAULT_CPUS` | `4` | Default CPU count |

## Development

### Building

```bash
# Build server
make build-server

# Build CLI
make build-cli

# Build all
make build
```

### Testing

```bash
# Run all tests
make test

# Run tests with coverage
make test-coverage

# Run tests in watch mode (requires entr)
make test-watch
```

### Code Quality

```bash
# Run linter (requires golangci-lint)
make lint

# Format code
make fmt

# Run vet
make vet
```

## Architecture

### RuntimeAdapter Interface

The `RuntimeAdapter` interface abstracts Docker and QEMU runtimes:

```go
type RuntimeAdapter interface {
    Create(ctx context.Context, spec *SandboxSpec) (string, error)
    Start(ctx context.Context, id string) error
    Stop(ctx context.Context, id string) error
    Delete(ctx context.Context, id string) error
    Exec(ctx context.Context, id string, req *ExecRequest) (*ExecResponse, error)
    Status(ctx context.Context, id string) (*SandboxStatus, error)
    List(ctx context.Context) ([]*SandboxStatus, error)
}
```

### Sandbox Manager

The `Manager` handles sandbox lifecycle:
- Create: Validates spec and creates sandbox
- Start: Transitions to running state
- Stop: Gracefully stops sandbox
- Delete: Removes sandbox and resources

### Security Model

All sandboxes are created with:
- Network isolation (default: no network)
- Resource limits (CPU, memory, PIDs)
- Capability dropping (Docker: `--cap-drop ALL`)
- Seccomp syscall filtering
- Read-only root filesystem (Docker)
- No-new-privileges flag

## Implementation Status

### Completed
- [x] Domain models (Sandbox, SandboxSpec, Resources)
- [x] Sandbox Manager with lifecycle operations
- [x] Configuration loading and validation
- [x] HTTP server with middleware
- [x] REST API handlers (stubbed)
- [x] CLI tool skeleton
- [x] Go client library
- [x] RuntimeAdapter interface
- [x] Comprehensive test suite (80%+ coverage)

### TODO
- [ ] Docker adapter implementation
- [ ] QEMU adapter implementation
- [ ] Command execution (exec endpoint)
- [ ] Log streaming
- [ ] Resource statistics
- [ ] Authentication/authorization
- [ ] TLS support
- [ ] Persistent storage for sandbox state
- [ ] Event logging/auditing
- [ ] Metrics and monitoring

## Next Steps

1. **Implement Docker Adapter**
   - Initialize Docker client
   - Implement container lifecycle operations
   - Apply security hardening from bash script
   - Add integration tests (requires Docker daemon)

2. **Implement QEMU Adapter**
   - Initialize libvirt connection
   - Load and customize VM templates
   - Implement VM lifecycle operations
   - Add integration tests (requires libvirt)

3. **Add Exec Support**
   - Implement command execution via Docker exec
   - Implement command execution via qemu-guest-agent
   - Add streaming output support

4. **Add Monitoring**
   - Implement resource statistics
   - Add log streaming
   - Add event logging

## References

- Bash launcher: `scripts/sandbox-launch.sh`
- Docker hardening: `.aiwg/spikes/spike-002-docker-hardening.md`
- Runtime abstraction: `.aiwg/spikes/spike-004-runtime-abstraction.md`
- API design: `docs/architecture/recommended-design.md`

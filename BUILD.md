# Build Infrastructure

Build, test, and development documentation for agentic-sandbox.

## Prerequisites

- **Rust 1.75+** - [Install Rust](https://rustup.rs/)
- **protobuf compiler** - `apt install protobuf-compiler` (Ubuntu) or `brew install protobuf` (macOS)
- **QEMU/KVM** - For VM runtime (see [VM Prerequisites](images/qemu/README.md))
- **libvirt** - VM management
- **Python 3.11+** - For E2E tests

## Quick Start

```bash
# Build management server
cd management && cargo build --release

# Build agent client
cd agent-rs && cargo build --release

# Build CLI (optional)
cd cli && cargo build --release

# Run management server in development mode
cd management && ./dev.sh
```

## Project Components

| Component | Location | Binary | Purpose |
|-----------|----------|--------|---------|
| Management Server | `management/` | `agentic-mgmt` | gRPC server, WebSocket streaming, HTTP dashboard |
| Agent Client | `agent-rs/` | `agent-client` | Runs inside VMs, connects to management server |
| CLI | `cli/` | `agentic-sandbox` | Command-line VM management |
| VM Images | `images/qemu/` | N/A | VM provisioning scripts |

## Management Server

The management server is written in Rust and provides three interfaces:

| Port | Protocol | Purpose |
|------|----------|---------|
| 8120 | gRPC | Agent client connections |
| 8121 | WebSocket | Real-time UI streaming (metrics, terminal) |
| 8122 | HTTP | Dashboard and REST API |

### Development

```bash
cd management

# Build and start (auto-builds if needed)
./dev.sh

# Force rebuild and start
./dev.sh build

# Stop server
./dev.sh stop

# Restart (stop, rebuild, start)
./dev.sh restart

# View logs
./dev.sh logs
```

The dev.sh script:
- Builds release binary via `cargo build --release`
- Stores PID in `.run/mgmt.pid`
- Logs to `.run/mgmt.log`
- Uses `.run/secrets/` for agent authentication

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LISTEN_ADDR` | `0.0.0.0:8120` | gRPC listen address |
| `SECRETS_DIR` | `.run/secrets` | Directory for agent-hashes.json |
| `HEARTBEAT_TIMEOUT` | `90` | Seconds before marking agent disconnected |
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |

Override via environment or `.run/dev.env`.

### Build for Production

```bash
cd management
cargo build --release

# Binary at: target/release/agentic-mgmt
```

## Agent Client

The agent client runs inside VMs and maintains a gRPC connection to the management server.

### Build

```bash
cd agent-rs
cargo build --release

# Binary at: target/release/agent-client
# Optimized for size (~4MB stripped)
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `AGENT_ID` | Yes | Unique identifier for this agent |
| `AGENT_SECRET` | Yes | Shared secret for authentication |
| `MANAGEMENT_SERVER` | Yes | Server address (e.g., `192.168.122.1:8120`) |
| `HEARTBEAT_INTERVAL` | No | Seconds between heartbeats (default: 30) |

### Deployment

The agent client is deployed to VMs during provisioning. See [VM Provisioning](images/qemu/README.md).

## CLI

Command-line tool for VM management.

```bash
cd cli
cargo build --release

# Binary at: target/release/agentic-sandbox
```

Usage:
```bash
agentic-sandbox status           # List connected agents
agentic-sandbox exec <agent> -- <cmd>  # Execute command on agent
```

## VM Provisioning

VMs are provisioned using the provision-vm.sh script. This is the primary way to create agent environments.

```bash
# Provision a development VM
./images/qemu/provision-vm.sh my-agent \
  --profile agentic-dev \
  --agentshare \
  --start

# See full documentation
./images/qemu/provision-vm.sh --help
```

See [images/qemu/README.md](images/qemu/README.md) for detailed provisioning documentation.

## Testing

### Unit Tests

```bash
# Management server
cd management && cargo test

# Agent client
cd agent-rs && cargo test
```

### E2E Tests

End-to-end tests validate the complete system using pytest.

```bash
# Prerequisites
cd tests/e2e
pip install -r requirements.txt  # or: uv pip install -r requirements.txt

# Run E2E tests (requires built binaries)
./scripts/run-e2e-tests.sh

# Or run directly
pytest tests/e2e/ -v
```

E2E tests:
- Start management server on dynamic ports
- Spawn Rust and Python agent clients
- Test WebSocket streaming
- Validate agent registration and heartbeats

Required binaries:
- `management/target/release/agentic-mgmt`
- `agent-rs/target/release/agent-client`

## Directory Structure

```
agentic-sandbox/
├── management/           # Management server (Rust)
│   ├── src/             # Server source code
│   ├── ui/              # Web dashboard assets
│   ├── dev.sh           # Development script
│   └── Cargo.toml
├── agent-rs/            # Agent client (Rust)
│   ├── src/             # Client source code
│   └── Cargo.toml
├── cli/                 # CLI tool (Rust)
│   ├── src/             # CLI source code
│   └── Cargo.toml
├── proto/               # gRPC protocol definitions
│   └── agent.proto
├── images/qemu/         # VM provisioning
│   ├── provision-vm.sh  # Main provisioning script
│   └── README.md        # Provisioning documentation
├── scripts/             # Utility scripts
│   ├── destroy-vm.sh    # Clean VM teardown
│   ├── reprovision-vm.sh
│   └── run-e2e-tests.sh
├── tests/
│   ├── e2e/             # E2E integration tests (pytest)
│   └── integration/     # Integration tests
└── configs/             # Security profiles (seccomp)
```

## Secrets Management

Agent authentication uses SHA256 hashed secrets.

### Development Setup

```bash
mkdir -p management/.run/secrets

# Create agent-hashes.json
echo '{"my-agent": "<sha256-hash-of-secret>"}' > management/.run/secrets/agent-hashes.json
```

### Production Setup

Secrets are provisioned via cloud-init during VM creation. The management server reads from `SECRETS_DIR/agent-hashes.json`.

Generate a hash:
```bash
echo -n "your-64-char-secret" | sha256sum | cut -d' ' -f1
```

## Development Workflow

1. **Build management server**: `cd management && cargo build --release`
2. **Start management server**: `./dev.sh`
3. **Provision test VM**: `./images/qemu/provision-vm.sh test-01 --profile agentic-dev --agentshare --start`
4. **Verify agent connects**: Open http://localhost:8122 or `curl http://localhost:8122/api/v1/agents`
5. **Run E2E tests**: `./scripts/run-e2e-tests.sh`

## Troubleshooting

### Rust Build Issues

```bash
# Clean and rebuild
cargo clean
cargo build --release

# Update dependencies
cargo update
```

### Protobuf Compilation Errors

Ensure protobuf compiler is installed:
```bash
protoc --version  # Should be 3.x or higher
```

### Agent Not Connecting

1. Check management server is running: `curl http://localhost:8122/api/v1/health`
2. Verify secret matches: hash in `agent-hashes.json` must match SHA256 of agent's secret
3. Check network: agent must reach management server on port 8120

### E2E Test Failures

```bash
# Ensure binaries are built
cd management && cargo build --release
cd agent-rs && cargo build --release

# Run with verbose output
pytest tests/e2e/ -v --tb=long
```

## Performance Notes

### Build Times

- Management server (release): ~45 seconds
- Agent client (release): ~30 seconds
- Full rebuild from clean: ~90 seconds

### Binary Sizes

- `agentic-mgmt`: ~15MB (with embedded UI assets)
- `agent-client`: ~4MB (optimized for size, stripped)

### Resource Usage

Management server:
- Memory: ~30MB at idle, ~50MB with active agents
- CPU: Negligible at idle

Agent client:
- Memory: ~10MB
- CPU: Negligible (heartbeat every 30s)

## CI/CD

CI pipeline is defined in `.gitea/workflows/ci.yaml`:

1. **Build** - Compile all Rust components
2. **Test** - Run unit tests
3. **E2E** - Run integration tests (requires QEMU)
4. **Lint** - cargo clippy

## References

- [Rust Documentation](https://doc.rust-lang.org/)
- [Tonic gRPC](https://github.com/hyperium/tonic)
- [libvirt Documentation](https://libvirt.org/docs.html)
- [QEMU Documentation](https://www.qemu.org/documentation/)

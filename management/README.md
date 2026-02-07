# Agentic Sandbox Management Server

High-performance Rust control plane for coordinating agent VMs. Handles agent registration, command dispatch, PTY streaming, and real-time dashboard.

## Quick Start (Development)

```bash
cd management/
./dev.sh          # Build and start
./dev.sh logs     # Tail logs in another terminal
```

Dashboard: http://localhost:8122

## Ports

| Port | Protocol  | Purpose                           |
|------|-----------|-----------------------------------|
| 8120 | gRPC      | Agent connections (bidirectional) |
| 8121 | WebSocket | Real-time UI streaming            |
| 8122 | HTTP      | Dashboard and REST API            |

Ports are consecutive from `LISTEN_ADDR` base port.

## Prerequisites

- **Rust 1.75+** with cargo
- **protoc** (Protocol Buffers compiler)

Install protoc:
```bash
# Ubuntu/Debian
sudo apt install -y protobuf-compiler

# macOS
brew install protobuf

# Verify
protoc --version
```

## Building

```bash
cd management/

# Debug build (faster compile, slower runtime)
cargo build

# Release build (slower compile, optimized runtime)
cargo build --release
```

Binary location:
- Debug: `target/debug/agentic-mgmt`
- Release: `target/release/agentic-mgmt`

## Running

### Development Mode

The `dev.sh` script handles the full lifecycle:

```bash
./dev.sh              # Build if needed, start server
./dev.sh build        # Force rebuild, then start
./dev.sh stop         # Stop running instance
./dev.sh restart      # Stop → rebuild → start
./dev.sh logs         # Tail log file
```

Runtime state stored in `.run/` (gitignored):
- `.run/mgmt.pid` — process ID
- `.run/mgmt.log` — stdout/stderr log
- `.run/secrets/` — agent token hashes
- `.run/dev.env` — optional env overrides

### Manual Start

```bash
# With defaults (ports 8120-8122, secrets in /etc/agentic-sandbox/secrets)
./target/release/agentic-mgmt

# With custom config
LISTEN_ADDR=0.0.0.0:9000 \
SECRETS_DIR=./my-secrets \
RUST_LOG=debug \
./target/release/agentic-mgmt
```

## Configuration

### Environment Variables

| Variable            | Default                          | Description                    |
|---------------------|----------------------------------|--------------------------------|
| `LISTEN_ADDR`       | `0.0.0.0:8120`                   | gRPC bind address (base port)  |
| `SECRETS_DIR`       | `/etc/agentic-sandbox/secrets`   | Agent token hash directory     |
| `HEARTBEAT_TIMEOUT` | `90`                             | Agent heartbeat timeout (sec)  |
| `RUST_LOG`          | `info`                           | Log level filter               |

### Config File

If `/etc/agentic-sandbox/management.env` exists, it's loaded at startup:

```bash
# /etc/agentic-sandbox/management.env
LISTEN_ADDR=0.0.0.0:8120
SECRETS_DIR=/etc/agentic-sandbox/secrets
HEARTBEAT_TIMEOUT=90
```

For development, use `.run/dev.env` instead (automatically loaded by `dev.sh`).

## Agent Authentication

Agents authenticate via gRPC metadata headers:
- `x-agent-id`: Agent identifier
- `x-agent-secret`: 64-char hex token (validated against SHA256 hash)

### Automatic Provisioning

When VMs are provisioned with `provision-vm.sh`, secrets are automatically created:

1. **Secret generated**: 32-byte random hex string (64 chars)
2. **Hash computed**: SHA256 of the secret
3. **Host storage**: Hash stored in `SECRETS_DIR/agent-hashes.json`
4. **VM injection**: Plaintext secret injected into `/etc/agentic-sandbox/agent.env`

The management server reads `agent-hashes.json` at startup and validates incoming secrets against stored hashes.

### Manual Secret Setup (Dev Mode)

For development without provisioning:

```bash
mkdir -p management/.run/secrets

# Generate a secret and its hash
SECRET=$(openssl rand -hex 32)
HASH=$(echo -n "$SECRET" | sha256sum | cut -d' ' -f1)

# Store in agent-hashes.json
echo "{\"test-agent\": \"$HASH\"}" > management/.run/secrets/agent-hashes.json

# Use $SECRET in your agent client
echo "AGENT_SECRET=$SECRET"
```

### Production Secrets File

```json
{
  "agent-01": "sha256-hash-of-secret",
  "agent-02": "sha256-hash-of-secret"
}
```

Location: `/var/lib/agentic-sandbox/secrets/agent-hashes.json` (production) or `.run/secrets/agent-hashes.json` (development)

## REST API

| Endpoint           | Method | Description          |
|--------------------|--------|----------------------|
| `/api/health`      | GET    | Health check         |
| `/api/v1/health`   | GET    | Health check (v1)    |
| `/api/v1/agents`   | GET    | List connected agents|

## WebSocket API

Connect to `ws://host:8121` for real-time streaming.

### Client → Server Messages

```json
{"type": "subscribe", "agent_id": "*"}
{"type": "subscribe", "agent_id": "agent-01"}
{"type": "unsubscribe", "agent_id": "agent-01"}
{"type": "list_agents"}
{"type": "start_shell", "agent_id": "agent-01", "cols": 80, "rows": 24}
{"type": "send_input", "agent_id": "agent-01", "command_id": "...", "data": "ls\n"}
{"type": "pty_resize", "agent_id": "agent-01", "command_id": "...", "cols": 120, "rows": 40}
{"type": "ping", "timestamp": 1234567890}
```

### Server → Client Messages

```json
{"type": "output", "agent_id": "agent-01", "stream": "stdout", "data": "..."}
{"type": "shell_started", "agent_id": "agent-01", "command_id": "uuid"}
{"type": "metrics_update", "agent_id": "agent-01", "cpu_percent": 12.5, ...}
{"type": "agent_list", "agents": [...]}
{"type": "pong", "timestamp": 1234567890}
{"type": "error", "message": "..."}
```

## Production Deployment

### Systemd Service

Create `/etc/systemd/system/agentic-mgmt.service`:

```ini
[Unit]
Description=Agentic Sandbox Management Server
After=network.target

[Service]
Type=simple
User=agentic
Group=agentic
EnvironmentFile=/etc/agentic-sandbox/management.env
ExecStart=/usr/local/bin/agentic-mgmt
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable and start:
```bash
sudo cp target/release/agentic-mgmt /usr/local/bin/
sudo mkdir -p /etc/agentic-sandbox/secrets
sudo systemctl daemon-reload
sudo systemctl enable --now agentic-mgmt
```

### Docker

```bash
cd deploy/docker/
docker build -f Dockerfile.management -t agentic-mgmt:latest ../..
docker run -d \
  -p 8120:8120 -p 8121:8121 -p 8122:8122 \
  -v /path/to/secrets:/etc/agentic-sandbox/secrets \
  agentic-mgmt:latest
```

## Troubleshooting

### Port already in use

```bash
# Find process using port
ss -tlnp | grep 8120
# or
lsof -i :8120

# Kill and restart
pkill -f agentic-mgmt
./dev.sh
```

### Agent not connecting

1. Check agent logs on VM: `journalctl -u agent-client -f`
2. Verify network: `curl -v http://mgmt-server:8122/api/health`
3. Check firewall: ports 8120-8122 must be open
4. Verify gRPC: `grpcurl -plaintext mgmt-server:8120 list`

### High memory usage

- Each agent uses ~10KB idle
- Output buffering can grow under load
- Set `RUST_LOG=warn` to reduce log overhead

## Architecture

See [docs/management-server-design.md](../docs/management-server-design.md) for detailed architecture documentation.

## Development

### Project Structure

```
management/
├── src/
│   ├── main.rs          # Entry point, server startup
│   ├── config.rs        # Configuration loading
│   ├── grpc.rs          # gRPC service implementation
│   ├── registry.rs      # Agent registry (DashMap)
│   ├── auth.rs          # Token validation
│   ├── dispatch/        # Command dispatcher
│   ├── output.rs        # Output aggregator
│   ├── ws/              # WebSocket hub
│   └── http/            # HTTP dashboard server
├── ui/                  # Web dashboard (embedded)
│   ├── app.js           # Dashboard JavaScript
│   ├── index.html       # Dashboard HTML
│   ├── styles.css       # Dashboard CSS
│   └── vendor/          # xterm.js 6.0, addon-fit
├── dev.sh               # Development runner
├── build.rs             # Proto compilation
├── Cargo.toml           # Dependencies
└── README.md            # This file
```

### Rebuilding After UI Changes

The UI is embedded at compile time via `rust-embed`. After modifying files in `ui/`:

```bash
./dev.sh restart   # Rebuilds and restarts
```

### Running Tests

```bash
cargo test
```

### E2E Tests

```bash
cd ..
./scripts/run-e2e-tests.sh
```

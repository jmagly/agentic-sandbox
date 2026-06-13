# Management Server Design Document

High-performance Rust server for managing agentic sandbox VMs.

## Executive Summary

The management server is the central control plane for all agent VMs. It must handle:
- Hundreds of concurrent agent connections
- Real-time bidirectional streaming at wire speed
- Command dispatch with microsecond latency
- Output aggregation and forwarding to monitoring UIs

**Language:** Rust (async with Tokio)
**Protocol:** gRPC with TLS and token auth
**Concurrency Model:** Multi-threaded async, lock-free where possible

## Architecture Overview

```
                                    ┌─────────────────────────────────────┐
                                    │       Management Server (Rust)      │
                                    │                                     │
┌──────────────┐   gRPC/TLS        │  ┌─────────────────────────────┐   │
│   Agent VM   │◄─────────────────►│  │     Agent Handler Task      │   │
│   agent-01   │                   │  │  (per-connection async task) │   │
└──────────────┘                   │  └──────────────┬──────────────┘   │
                                   │                 │                   │
┌──────────────┐   gRPC/TLS        │  ┌─────────────▼──────────────┐   │
│   Agent VM   │◄─────────────────►│  │     Agent Registry         │   │
│   agent-02   │                   │  │  (DashMap - lock-free)     │   │
└──────────────┘                   │  └──────────────┬──────────────┘   │
                                   │                 │                   │
       ...                         │  ┌─────────────▼──────────────┐   │
                                   │  │    Command Dispatcher       │   │
┌──────────────┐   gRPC/TLS        │  │  (mpsc channels per agent)  │   │
│   Agent VM   │◄─────────────────►│  └──────────────┬──────────────┘   │
│   agent-N    │                   │                 │                   │
└──────────────┘                   │  ┌─────────────▼──────────────┐   │
                                   │  │    Output Aggregator        │   │
                                   │  │  (broadcast to subscribers) │   │
                                   │  └──────────────┬──────────────┘   │
                                   │                 │                   │
                                   │  ┌─────────────▼──────────────┐   │
┌──────────────┐   WebSocket       │  │    WebSocket Hub            │   │
│  Monitor UI  │◄─────────────────►│  │  (real-time UI streaming)  │   │
└──────────────┘                   │  └─────────────────────────────┘   │
                                   │                                     │
                                   │  Port 8120: gRPC (agents)          │
                                   │  Port 8121: HTTP/WS (UI)           │
                                   └─────────────────────────────────────┘
```

## Core Components

### 1. gRPC Server (`src/grpc/`)

**Responsibility:** Accept agent connections, handle bidirectional streaming.

```rust
// Simplified structure
struct AgentService {
    registry: Arc<AgentRegistry>,
    transport_identity_resolver: AgentTransportIdentityResolver,
    output_tx: broadcast::Sender<OutputEvent>,
}

#[tonic::async_trait]
impl agent_service_server::AgentService for AgentService {
    type ConnectStream = ReceiverStream<Result<ManagementMessage, Status>>;

    async fn connect(
        &self,
        request: Request<Streaming<AgentMessage>>,
    ) -> Result<Response<Self::ConnectStream>, Status> {
        // 1. Validate transport identity against metadata
        // 2. Register agent
        // 3. Spawn handler task
        // 4. Return outbound stream
    }
}
```

**Performance Considerations:**
- One async task per agent connection (cheap on Tokio)
- Zero-copy message passing where possible
- Backpressure handling via bounded channels

### 2. Agent Registry (`src/registry/`)

**Responsibility:** Track connected agents, their state, and capabilities.

```rust
// Lock-free concurrent map
struct AgentRegistry {
    agents: DashMap<String, AgentState>,
    by_ip: DashMap<IpAddr, String>,
}

struct AgentState {
    id: String,
    ip: IpAddr,
    status: AgentStatus,
    connected_at: DateTime<Utc>,
    last_heartbeat: AtomicU64,  // Atomic for lock-free updates
    command_tx: mpsc::Sender<CommandRequest>,
    metrics: AgentMetrics,
}
```

**Operations:**
- `register(agent_id, state)` - O(1) insert
- `get(agent_id)` - O(1) lookup
- `remove(agent_id)` - O(1) removal
- `list_all()` - Snapshot iteration
- `by_status(status)` - Filtered iteration

### 3. Agent Transport Identity (`src/transport_identity.rs`)

**Responsibility:** Normalize verified transport evidence into the agent
instance-id keyspace. Legacy shared-secret authentication was retired in #412.

```rust
struct AgentTransportIdentityResolver {
    trust_domain: TrustDomain,
    peer_map: PeerIdentityMap,
}

impl AgentTransportIdentityResolver {
    fn peer_identity(&self, evidence: PeerIdentityEvidence) -> Result<SpiffeId> {
        // UDS uid, vsock CID, and mTLS URI-SAN resolve to
        // spiffe://<trust-domain>/agent/<instance-id>.
    }
}
```

**Security:**
- Plain TCP has no transport identity and is rejected.
- mTLS agents present a SPIFFE URI-SAN client certificate.
- UDS and vsock transports map kernel-provided peer evidence to instance ids.

### 4. Command Dispatcher (`src/dispatch/`)

**Responsibility:** Route commands to agents, track pending executions.

```rust
struct CommandDispatcher {
    registry: Arc<AgentRegistry>,
    pending: DashMap<String, PendingCommand>,  // command_id -> state
}

struct PendingCommand {
    agent_id: String,
    command: CommandRequest,
    started_at: Instant,
    result_tx: oneshot::Sender<CommandResult>,
}

impl CommandDispatcher {
    async fn dispatch(&self, agent_id: &str, cmd: CommandRequest)
        -> Result<CommandResult, DispatchError>
    {
        let agent = self.registry.get(agent_id)?;
        let (result_tx, result_rx) = oneshot::channel();

        let cmd_id = cmd.command_id.clone();
        self.pending.insert(cmd_id.clone(), PendingCommand {
            agent_id: agent_id.to_string(),
            command: cmd.clone(),
            started_at: Instant::now(),
            result_tx,
        });

        // Send to agent's command channel
        agent.command_tx.send(cmd).await?;

        // Wait for result with timeout
        tokio::time::timeout(
            Duration::from_secs(cmd.timeout_seconds as u64),
            result_rx
        ).await??
    }
}
```

### 5. Output Aggregator (`src/output/`)

**Responsibility:** Collect output streams, broadcast to subscribers.

```rust
struct OutputAggregator {
    // Broadcast channel for all output events
    tx: broadcast::Sender<OutputEvent>,
    // Per-agent output buffers (ring buffers for recent history)
    buffers: DashMap<String, RingBuffer<OutputChunk>>,
}

enum OutputEvent {
    Stdout { agent_id: String, data: Bytes, timestamp: u64 },
    Stderr { agent_id: String, data: Bytes, timestamp: u64 },
    Log { agent_id: String, data: Bytes, timestamp: u64 },
    Result { agent_id: String, result: CommandResult },
}

impl OutputAggregator {
    fn subscribe(&self) -> broadcast::Receiver<OutputEvent> {
        self.tx.subscribe()
    }

    fn subscribe_agent(&self, agent_id: &str) -> impl Stream<Item = OutputEvent> {
        self.tx.subscribe()
            .filter(move |e| e.agent_id() == agent_id)
    }
}
```

### 6. WebSocket Hub (`src/ws/`)

**Responsibility:** Serve real-time output to monitoring UIs.

```rust
struct WebSocketHub {
    output_rx: broadcast::Receiver<OutputEvent>,
    connections: DashMap<Uuid, WsConnection>,
}

// Each WS connection can subscribe to:
// - All agents (admin view)
// - Specific agent(s) (focused view)
// - Specific streams (stdout only, etc.)

struct WsConnection {
    id: Uuid,
    tx: mpsc::Sender<WsMessage>,
    filter: OutputFilter,
}
```

## Data Flow

### Agent Connect Flow

```
Agent                    gRPC Server              Registry
  │                           │                      │
  │── Connect() ─────────────►│                      │
  │   [UDS/vsock/mTLS]        │                      │
  │                           │                      │
  │── Registration ──────────►│                      │
  │   {id, instance_id}       │                      │
  │                           │── register(id, ─────►│
  │                           │    state)            │
  │                           │◄──── ok ────────────│
  │                           │                      │
  │◄── RegistrationAck ───────│                      │
  │   {accepted, config}      │                      │
  │                           │                      │
  ╠══════════════════════════╬═══ Stream Open ══════╣
```

### Command Execution Flow

```
CLI/API         Dispatcher          Registry          Agent Handler        Agent
   │                │                   │                   │                │
   │── exec(id, ───►│                   │                   │                │
   │    cmd)        │── get(id) ───────►│                   │                │
   │                │◄──── state ───────│                   │                │
   │                │                   │                   │                │
   │                │── send(cmd) ─────────────────────────►│                │
   │                │                   │                   │── cmd ────────►│
   │                │                   │                   │                │
   │                │                   │                   │◄── stdout ─────│
   │                │◄───────────────── stdout ─────────────│                │
   │   [stream]     │                   │                   │◄── stderr ─────│
   │◄── stdout ─────│◄───────────────── stderr ─────────────│                │
   │◄── stderr ─────│                   │                   │◄── result ─────│
   │                │◄───────────────── result ─────────────│                │
   │◄── result ─────│                   │                   │                │
   │                │                   │                   │                │
```

## Module Structure

```
management/
├── Cargo.toml
├── build.rs                    # Proto compilation
├── proto/
│   └── agent.proto             # Symlink to ../proto/agent.proto
└── src/
    ├── main.rs                 # Entry point, server setup
    ├── config.rs               # Configuration loading
    ├── error.rs                # Error types
    │
    ├── grpc/
    │   ├── mod.rs
    │   ├── server.rs           # gRPC server setup
    │   ├── service.rs          # AgentService implementation
    │   └── interceptor.rs      # Auth interceptor
    │
    ├── registry/
    │   ├── mod.rs
    │   ├── agent.rs            # AgentState
    │   └── registry.rs         # AgentRegistry
    │
    ├── transport_identity.rs   # SPIFFE-shaped agent identity resolver
    │
    ├── dispatch/
    │   ├── mod.rs
    │   └── dispatcher.rs       # CommandDispatcher
    │
    ├── output/
    │   ├── mod.rs
    │   └── aggregator.rs       # OutputAggregator
    │
    └── ws/
        ├── mod.rs
        └── hub.rs              # WebSocketHub
```

## Configuration

```toml
# /etc/agentic-sandbox/management.toml

[server]
grpc_port = 8120
http_port = 8121
bind_address = "0.0.0.0"

[tls]
enabled = true
cert_path = "/etc/agentic-sandbox/certs/server.crt"
key_path = "/etc/agentic-sandbox/certs/server.key"

[grpc_mtls]
listen = "0.0.0.0:8123"
cert_path = "/var/lib/agentic-sandbox/secrets/grpc-mtls/server.pem"
key_path = "/var/lib/agentic-sandbox/secrets/grpc-mtls/server-key.pem"
client_ca_path = "/var/lib/agentic-sandbox/secrets/grpc-local-ca/grpc-local-root-ca.pem"

[limits]
max_agents = 1000
max_concurrent_commands = 100
command_timeout_seconds = 3600
output_buffer_size = 1048576  # 1MB per agent

[logging]
level = "info"
format = "json"
```

## Performance Targets

| Metric | Target |
|--------|--------|
| Agent connections | 1000+ concurrent |
| Command latency (dispatch) | < 1ms |
| Output throughput | 100 MB/s aggregate |
| Memory per agent | < 10 KB idle, 1 MB with buffers |
| CPU (idle 100 agents) | < 5% |

## Implementation Phases

### Phase 1: Core Infrastructure
- [ ] Proto compilation setup
- [ ] Basic gRPC server with TLS
- [ ] Agent registry (in-memory)
- [ ] Secret store (file-backed)
- [ ] Single-agent connect flow

### Phase 2: Command Execution
- [ ] Command dispatcher
- [ ] Output streaming
- [ ] Result handling
- [ ] Timeout management

### Phase 3: Monitoring Integration
- [ ] Output aggregator with broadcast
- [ ] WebSocket hub
- [ ] Basic web UI endpoint
- [ ] Metrics endpoint (Prometheus)

### Phase 4: Production Hardening
- [ ] Connection pooling
- [ ] Graceful shutdown
- [ ] Health checks
- [ ] Rate limiting
- [ ] Audit logging

## Testing Strategy

1. **Unit tests** - Individual components with mocked dependencies
2. **Integration tests** - Full server with test agents
3. **Load tests** - 100+ simulated agents with k6 or custom harness
4. **Chaos tests** - Random disconnections, timeouts, failures

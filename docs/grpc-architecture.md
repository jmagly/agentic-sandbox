# gRPC Communication Architecture

Primary communication channel between management server and agent VMs.

## Overview

```
┌─────────────────────┐         gRPC (TLS + Token)        ┌──────────────────┐
│  Management Server  │◄═══════════════════════════════════►│   Agent VM       │
│     (Host)          │         Bidirectional Stream       │   (Ephemeral)    │
│                     │                                    │                  │
│  - Command dispatch │   Commands ──────────────────────► │  - Executor      │
│  - Output collector │   ◄─────────────── stdout/stderr   │  - Heartbeat     │
│  - Agent registry   │   ◄─────────────── logs/metrics    │  - Metrics       │
│  - Monitoring UX    │   ◄─────────────── results         │                  │
└─────────────────────┘                                    └──────────────────┘
        :8120                                                  connects out
```

## Security Model: Ephemeral Secrets

Each VM gets a unique secret generated at creation time:

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        VM Provisioning Flow                              │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. provision-vm.sh creates VM                                           │
│     ├── Generate random 256-bit secret: AGENT_SECRET=$(openssl rand -hex 32)
│     ├── Store in vm-info.json (host-side, secure)                        │
│     └── Inject into cloud-init (agent-side)                              │
│                                                                          │
│  2. VM boots, agent connects to management:8120                          │
│     ├── TLS connection (server cert verification)                        │
│     ├── Sends: agent_id + AGENT_SECRET in metadata                       │
│     └── Management validates against registered secrets                  │
│                                                                          │
│  3. Connection established with mutual authentication                    │
│     ├── Only pre-provisioned VMs can connect                             │
│     ├── Secret is single-use (agent_id bound)                            │
│     └── Revocation: delete from registry                                 │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

### Secret Generation (Host)

```bash
# In provision-vm.sh
AGENT_SECRET=$(openssl rand -hex 32)

# Store for management server
echo "$vm_name:$AGENT_SECRET" >> /var/lib/agentic-sandbox/secrets/agent-tokens

# Inject into cloud-init
cat >> cloud-init/user-data <<EOF
write_files:
  - path: /etc/agentic-sandbox/agent.env
    permissions: '0600'
    content: |
      AGENT_ID=$vm_name
      AGENT_SECRET=$AGENT_SECRET
      MANAGEMENT_SERVER=${MANAGEMENT_HOST:-host.internal}:8120
EOF
```

### Secret Usage (Agent)

```python
# Agent loads secret on boot
import os

agent_id = os.environ['AGENT_ID']
agent_secret = os.environ['AGENT_SECRET']

# Create authenticated channel
metadata = [
    ('x-agent-id', agent_id),
    ('x-agent-secret', agent_secret),
]

# All gRPC calls include metadata
stub.Connect(message_generator(), metadata=metadata)
```

### Secret Validation (Management)

```go
// Server interceptor validates every request
func AuthInterceptor(ctx context.Context) error {
    md, ok := metadata.FromIncomingContext(ctx)
    if !ok {
        return status.Error(codes.Unauthenticated, "missing metadata")
    }

    agentID := md.Get("x-agent-id")
    secret := md.Get("x-agent-secret")

    if !secretStore.Validate(agentID, secret) {
        return status.Error(codes.Unauthenticated, "invalid credentials")
    }

    return nil
}
```

## Protocol Messages

### Agent → Management

| Message Type | Purpose | Frequency |
|--------------|---------|-----------|
| `Registration` | Initial handshake with system info | Once on connect |
| `Heartbeat` | Status + basic metrics | Every 30s |
| `Stdout` | Command stdout stream | Real-time |
| `Stderr` | Command stderr stream | Real-time |
| `Log` | Agent log entries | As generated |
| `CommandResult` | Execution completion | Per command |
| `Metrics` | Detailed system metrics | Every 60s |

### Management → Agent

| Message Type | Purpose |
|--------------|---------|
| `RegistrationAck` | Accept/reject connection, provide config |
| `CommandRequest` | Execute shell command |
| `ConfigUpdate` | Update runtime config |
| `ShutdownSignal` | Graceful shutdown request |
| `Ping` | Keepalive check |

## Connection Lifecycle

```
Agent                                    Management
  │                                           │
  │──── TLS Connect ─────────────────────────►│
  │──── Registration + Secret ───────────────►│
  │                                           │ Validate secret
  │◄─── RegistrationAck (config) ────────────│
  │                                           │
  │◄═════════════════════════════════════════│ Bidirectional stream open
  │                                           │
  │──── Heartbeat ───────────────────────────►│
  │◄─── CommandRequest ──────────────────────│
  │──── Stdout chunks ───────────────────────►│
  │──── Stderr chunks ───────────────────────►│
  │──── CommandResult ───────────────────────►│
  │                                           │
  │◄─── ShutdownSignal ──────────────────────│
  │                                           │
  │──── Heartbeat (shutting_down) ───────────►│
  │                                           │
  ╳ Connection closed                         ╳
```

## Port Assignments

| Port | Service | Protocol |
|------|---------|----------|
| 8118 | Health check (secondary) | HTTP |
| 8119 | Checkin server (fallback) | HTTP |
| 8120 | Management gRPC | gRPC/TLS |

## Files

### Host Side

```
/var/lib/agentic-sandbox/
├── secrets/
│   └── agent-tokens           # agent_id:secret registry
├── vms/
│   └── agent-01/
│       ├── vm-info.json       # Includes agent_secret hash
│       └── ...
```

### Agent Side (VM)

```
/etc/agentic-sandbox/
├── agent.env                  # AGENT_ID, AGENT_SECRET, MANAGEMENT_SERVER
└── agent.conf                 # Additional config

/opt/agentic-sandbox/
├── bin/
│   └── agent-client           # gRPC client binary/script
└── logs/
    └── agent.log
```

## Fallback Behavior

If gRPC connection fails:

1. Agent retries with exponential backoff (5s, 10s, 20s... max 60s)
2. HTTP health server (8118) remains available for status checks
3. HTTP checkin (8119) provides basic registration fallback
4. SSH remains available for manual intervention

## Monitoring UX Integration

The management server exposes a WebSocket endpoint for the monitoring UI:

```
Management Server                         Monitoring UI
      │                                        │
      │◄──── WebSocket Connect ────────────────│
      │                                        │
      │ (Agent stdout arrives via gRPC)        │
      │                                        │
      │───── stdout: {"agent": "agent-01", ────►│
      │       "data": "...", "stream": "stdout"}│
      │                                        │
      │ (Real-time output display)             │
      │                                        │
```

This enables:
- Live output streaming in browser
- Multi-agent dashboard
- Command input/output terminal
- Metrics visualization

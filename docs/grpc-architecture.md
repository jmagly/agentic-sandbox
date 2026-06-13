# gRPC Communication Architecture

Primary communication channel between management server and agent VMs.

## Overview

```
┌─────────────────────┐         gRPC (mTLS)               ┌──────────────────┐
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

## Security Model: Secure Transport First

Secure transport provisions stage per-agent mTLS client material at creation
time. Legacy shared-secret authentication remains available only when the
compatibility path is explicitly enabled.

```
┌──────────────────────────────────────────────────────────────────────────┐
│                        VM Provisioning Flow                              │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  1. provision-vm.sh creates VM                                           │
│     ├── Stage client certificate + private key for this agent            │
│     ├── Write AGENT_TRANSPORT=auto and AGENT_GRPC_TLS_* paths            │
│     └── Inject cloud-init configuration without AGENT_SECRET             │
│                                                                          │
│  2. VM boots, agent connects to management:8120                          │
│     ├── TLS connection with client certificate                           │
│     ├── Presents the per-agent mTLS identity                             │
│     └── Management validates the client certificate                      │
│                                                                          │
│  3. Connection established with mutual authentication                    │
│     ├── Only provisioned identities can connect                          │
│     ├── Identity is agent-bound                                          │
│     └── Revocation: remove or rotate the issued identity                 │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

### Secure Transport Material (Host)

```bash
# In provision-vm.sh, secure transport path
install -m 0600 agent.pem agent-key.pem "$guest_mtls_dir/"

# Inject transport settings into cloud-init
cat >> cloud-init/user-data <<EOF
write_files:
  - path: /etc/agentic-sandbox/agent.env
    permissions: '0600'
    content: |
      AGENT_ID=$vm_name
      MANAGEMENT_SERVER=${MANAGEMENT_HOST:-host.internal}:8120
      AGENT_TRANSPORT=auto
      AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem
      AGENT_GRPC_TLS_CERT=/etc/agentic-sandbox/grpc-mtls/agent.pem
      AGENT_GRPC_TLS_KEY=/etc/agentic-sandbox/grpc-mtls/agent-key.pem
EOF
```

### Secure Transport Usage (Agent)

```bash
agent-client \
  --agent-id "$AGENT_ID" \
  --server "$MANAGEMENT_SERVER" \
  --transport auto \
  --tls-ca "$AGENT_GRPC_TLS_CA" \
  --tls-cert "$AGENT_GRPC_TLS_CERT" \
  --tls-key "$AGENT_GRPC_TLS_KEY"
```

### Legacy Secret Compatibility

Legacy TCP provisions can opt in to `AGENT_SECRET` plus `x-agent-secret`
metadata for one release while existing deployments migrate to secure
transport. New secure provisions omit shared secrets.

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

# Agentshare: Shared File System for Agentic Sandbox

Dedicated shared file system for agent VMs, separate from regular vmshare infrastructure.

## Purpose

Provides bidirectional file exchange between host and agent VMs:
- **Inject** scripts, tools, configs, and content into agents
- **Collect** outputs, logs, artifacts from agent runs
- **Audit** per-run history with full traceability

## Directory Structure

```
/srv/agentshare/
├── global/                    # Host → Agent (RO inside VM)
│   ├── tools/                 # Shared utilities (jq, scripts, etc.)
│   ├── prompts/               # System prompts and instructions
│   ├── configs/               # Configuration templates
│   └── content/               # Reference data, documents
│
├── {agent-name}-inbox/        # Agent → Host (RW inside VM)
│   ├── current/               # Symlink to latest run
│   ├── outputs/               # Agent-produced files
│   ├── logs/                  # Runtime logs (stdout, stderr, agent.log)
│   └── runs/                  # Per-run archives
│       ├── run-20260125-143022/
│       │   ├── stdout.log
│       │   ├── stderr.log
│       │   ├── agent.log
│       │   ├── outputs/
│       │   ├── metrics.json
│       │   └── trace/
│       └── run-20260125-151033/
│           └── ...
│
├── global-ro -> global        # RO symlink for VM mounts
└── staging/                   # Review area before promotion to global
```

## VM Mount Points

Inside agent VMs, agentshare is mounted at:

| Host Path | VM Mount | Access |
|-----------|----------|--------|
| `/srv/agentshare/global-ro` | `/mnt/global` | Read-only |
| `/srv/agentshare/{agent}-inbox` | `/mnt/inbox` | Read-write |

Home directory symlinks (for convenience):

```
/home/agent/
├── global -> /mnt/global     # RO shared content
├── inbox -> /mnt/inbox       # RW agent output
└── outputs -> /mnt/inbox/outputs  # Direct to outputs
```

## Data Flows

### Host → Agent (Injection)

```
Host: /srv/agentshare/global/tools/my-script.sh
  ↓ (virtiofs RO mount)
Agent: /mnt/global/tools/my-script.sh
Agent: ~/global/tools/my-script.sh (symlink)
```

Use cases:
- Deploy custom tools and utilities
- Provide system prompts and instructions
- Share configuration templates
- Inject reference documents

### Agent → Host (Collection)

```
Agent: echo "result" > ~/outputs/result.txt
  ↓ (virtiofs RW mount)
Host: /srv/agentshare/agent-01-inbox/outputs/result.txt
```

Use cases:
- Collect agent outputs and artifacts
- Stream logs for monitoring
- Archive run history
- Extract metrics and traces

## Per-Run Tracking

Each agent invocation creates a run directory:

```bash
# On agent boot, cloud-init creates:
RUN_ID="run-$(date +%Y%m%d-%H%M%S)"
mkdir -p /mnt/inbox/runs/$RUN_ID/{outputs,trace}
ln -sfn /mnt/inbox/runs/$RUN_ID /mnt/inbox/current

# Redirect logging:
exec > >(tee -a /mnt/inbox/runs/$RUN_ID/stdout.log) 2>&1
```

Run directory contents:
- `stdout.log` - Agent stdout
- `stderr.log` - Agent stderr
- `agent.log` - Application-level logs
- `outputs/` - Files produced by agent
- `metrics.json` - Resource usage, timing
- `trace/` - Detailed execution trace

## Permissions Model

```bash
# Host (root/operator)
/srv/agentshare/
├── global/               # root:root, 755 (host manages)
├── global-ro/            # symlink
├── staging/              # root:root, 770 (operator review)
└── {agent}-inbox/        # root:root, 777 (agent writes)
    └── runs/             # agent:agent inside VM
```

Inside VM:
- `/mnt/global` - UID 0 (root), mode 555
- `/mnt/inbox` - UID 1000 (agent), mode 755

## Libvirt Configuration

Virtiofs shares for agent VMs:

```xml
<filesystem type='mount' accessmode='passthrough'>
  <driver type='virtiofs'/>
  <source dir='/srv/agentshare/global-ro'/>
  <target dir='agentglobal'/>
  <readonly/>
</filesystem>

<filesystem type='mount' accessmode='passthrough'>
  <driver type='virtiofs'/>
  <source dir='/srv/agentshare/VMNAME-inbox'/>
  <target dir='agentinbox'/>
</filesystem>
```

Mount in cloud-init:

```yaml
mounts:
  - [agentglobal, /mnt/global, virtiofs, "ro,noatime", "0", "0"]
  - [agentinbox, /mnt/inbox, virtiofs, "rw,noatime", "0", "0"]
```

## Setup Commands

### Initialize agentshare (host)

```bash
sudo mkdir -p /srv/agentshare/{global,staging}
sudo mkdir -p /srv/agentshare/global/{tools,prompts,configs,content}
sudo ln -s global /srv/agentshare/global-ro
sudo chmod 755 /srv/agentshare/global
sudo chmod 770 /srv/agentshare/staging
```

### Create agent inbox (on VM provision)

```bash
AGENT_NAME="agent-01"
sudo mkdir -p /srv/agentshare/${AGENT_NAME}-inbox/{outputs,logs,runs}
sudo chmod 777 /srv/agentshare/${AGENT_NAME}-inbox
```

### Promote to global (host)

```bash
# Review file in staging
ls /srv/agentshare/staging/

# Promote with metadata
FILE="my-tool.sh"
sha256sum /srv/agentshare/staging/$FILE > /srv/agentshare/staging/$FILE.sha256
sudo cp -a /srv/agentshare/staging/$FILE* /srv/agentshare/global/tools/
sudo chmod 444 /srv/agentshare/global/tools/$FILE
```

## Differences from vmshare

| Aspect | vmshare | agentshare |
|--------|---------|------------|
| Purpose | Human VM file exchange | AI agent I/O |
| Root path | `/srv/vmshare/` | `/srv/agentshare/` |
| Global mount | `/mnt/global` | `/mnt/global` |
| Inbox mount | Varies by VM | `/mnt/inbox` |
| Per-run tracking | No | Yes |
| Log streaming | No | SSE via health server |
| Metrics collection | No | Yes (metrics.json) |

## Integration with Health Server

The health server (port 8118) provides streaming access to logs:

```bash
# From host, attach to agent logs
curl -N http://192.168.122.201:8118/stream/stdout

# Or use attach-vm.sh
./attach-vm.sh agent-01 stdout
./attach-vm.sh -c agent-01  # combined stdout+stderr
```

Log files in `/mnt/inbox/current/` are tailed and streamed via SSE.

## House Rules

1. **Never write to global from inside an agent** - use staging workflow
2. **Always use per-run directories** - enables rollback and audit
3. **Include metadata** - timestamps, source agent, validation status
4. **Clean up old runs** - implement retention policy
5. **Monitor inbox sizes** - prevent runaway agents filling storage

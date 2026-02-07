# Prometheus Observability for Agentic Sandbox

Comprehensive observability implementation with Prometheus, Grafana, and Alertmanager for the agentic-sandbox VM orchestration platform.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                       Host System                            │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │            Prometheus Server (localhost:9090)        │   │
│  │  • Scrapes management server metrics                 │   │
│  │  • Scrapes agent VM node_exporter metrics            │   │
│  │  • Evaluates alert rules                             │   │
│  │  • Sends alerts to Alertmanager                      │   │
│  └──────────────────────────────────────────────────────┘   │
│                            │                                  │
│  ┌──────────────────────────────────────────────────────┐   │
│  │        Alertmanager (localhost:9093)                 │   │
│  │  • Routes alerts by severity                         │   │
│  │  • Sends to Slack and PagerDuty                      │   │
│  │  • Inhibits redundant alerts                         │   │
│  └──────────────────────────────────────────────────────┘   │
│                            │                                  │
│  ┌──────────────────────────────────────────────────────┐   │
│  │            Grafana (localhost:3000)                  │   │
│  │  • Visualizes Prometheus metrics                     │   │
│  │  • Pre-built dashboards                              │   │
│  │  • SLO tracking and error budgets                    │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Components

### 1. Management Server Metrics

The management server exposes Prometheus metrics at `http://localhost:8122/metrics`.

**Enhanced Metrics:**
- `agentic_agent_sessions_active{agent_id}` - Current active sessions per agent
- `agentic_agent_session_duration_seconds_sum{agent_id}` - Total session time
- `agentic_agent_restarts_total{agent_id}` - Agent restart count
- `agentic_agentshare_inbox_bytes{agent_id}` - Per-agent inbox storage usage
- `agentic_command_latency_seconds_bucket{le}` - Command latency histogram
- `agentic_task_outcomes_total{outcome}` - Task outcomes (success/failure/timeout)

See `management/src/telemetry/metrics.rs` for full implementation.

### 2. Agent VM Metrics

Each agent VM runs `node_exporter` on port 9100, exposing system metrics:
- CPU usage (per-core and aggregate)
- Memory usage (total, available, cached, swap)
- Disk usage (filesystem space, I/O stats)
- Network traffic (bytes sent/received, errors)

**Custom Agent Metrics:**
Agent VMs also write custom application metrics to `/var/lib/prometheus/node-exporter/agent.prom` via the textfile collector:
- `agentic_agent_commands_total{agent_id}` - Commands executed
- `agentic_agent_commands_success{agent_id}` - Successful commands
- `agentic_agent_commands_failed{agent_id}` - Failed commands
- `agentic_agent_claude_tasks_total{agent_id}` - Claude tasks
- `agentic_agent_pty_sessions_total{agent_id}` - PTY sessions
- `agentic_agent_current_commands{agent_id}` - Active commands (gauge)
- `agentic_agent_last_command_latency_ms{agent_id}` - Last command latency

See `agent-rs/src/metrics_exporter.rs` for implementation.

## Installation

### Prerequisites

```bash
# Install Prometheus
sudo apt update
sudo apt install -y prometheus prometheus-alertmanager

# Install Grafana
wget -q -O - https://packages.grafana.com/gpg.key | sudo apt-key add -
echo "deb https://packages.grafana.com/oss/deb stable main" | sudo tee /etc/apt/sources.list.d/grafana.list
sudo apt update
sudo apt install -y grafana

# Start services
sudo systemctl enable prometheus alertmanager grafana-server
sudo systemctl start prometheus alertmanager grafana-server
```

### Configuration

1. **Copy Prometheus configuration:**

```bash
sudo cp prometheus.yml /etc/prometheus/prometheus.yml
sudo cp rules/agentic-sandbox.yml /etc/prometheus/rules/agentic-sandbox.yml
sudo chown prometheus:prometheus /etc/prometheus/prometheus.yml
sudo chown prometheus:prometheus /etc/prometheus/rules/agentic-sandbox.yml
```

2. **Copy Alertmanager configuration:**

```bash
sudo cp alertmanager.yml /etc/alertmanager/alertmanager.yml
sudo chown prometheus:prometheus /etc/alertmanager/alertmanager.yml

# Edit to add your Slack webhook URL
sudo nano /etc/alertmanager/alertmanager.yml
# Replace YOUR/WEBHOOK/URL with your actual webhook
```

3. **Restart services:**

```bash
sudo systemctl restart prometheus alertmanager
```

4. **Verify targets are up:**

```bash
# Check Prometheus targets
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | {job, health}'

# Check alerts
curl http://localhost:9090/api/v1/alerts | jq '.data.alerts'
```

### Agent VM Setup

1. **Install node_exporter on agent VMs:**

Add to `images/qemu/profiles/agentic-dev/packages.txt`:
```
prometheus-node-exporter
```

2. **Configure textfile collector:**

On each agent VM:
```bash
sudo mkdir -p /var/lib/prometheus/node-exporter
sudo chown agent:agent /var/lib/prometheus/node-exporter

# Enable textfile collector (already enabled by default)
sudo systemctl restart prometheus-node-exporter
```

3. **Verify node_exporter is running:**

```bash
curl http://192.168.122.201:9100/metrics | head -20
```

## Alert Rules

### Severity Levels

| Severity | Response Time | Notification Channel | Example |
|----------|--------------|---------------------|---------|
| **WARNING** | Best-effort, business hours | Slack #alerts | Agent CPU > 80% for 10m |
| **CRITICAL** | Page on-call engineer | PagerDuty + Slack | Management server down |
| **EMERGENCY** | Page incident commander | PagerDuty + Slack #incidents + SMS | Error budget < 10% |

### Alert Groups

1. **agent-health** - Agent VM resource monitoring
   - `AgentHighCPU` - CPU > 80% for 10m (WARNING)
   - `AgentHighMemory` - Memory > 85% for 10m (WARNING)
   - `AgentDiskFull` - Disk > 90% for 5m (CRITICAL)
   - `AgentDisconnected` - Agent unreachable for 2m (CRITICAL)

2. **command-execution** - Command processing monitoring
   - `HighCommandFailureRate` - > 5% failures for 10m (WARNING)
   - `CommandExecutionStalled` - No commands despite running tasks (CRITICAL)
   - `SlowCommandExecution` - P95 latency > 5s for 10m (WARNING)

3. **task-orchestration** - Task lifecycle monitoring
   - `HighTaskFailureRate` - > 10% failures for 30m (WARNING)
   - `TaskQueueBacklog` - > 10 pending tasks for 15m (CRITICAL)
   - `HighTaskTimeoutRate` - > 5% timeouts for 30m (WARNING)

4. **management-server** - Server health monitoring
   - `ManagementServerDown` - Unreachable for 2m (CRITICAL)
   - `HighGrpcErrorRate` - > 5% gRPC errors for 10m (WARNING)
   - `HighWebSocketChurn` - > 10 new connections/s for 15m (WARNING)

5. **slo-violations** - SLO compliance monitoring
   - `AgentAvailabilitySLOViolation` - < 99% availability for 30m (CRITICAL)
   - `CommandSuccessRateSLOViolation` - < 99% success rate for 30m (CRITICAL)
   - `TaskSuccessRateSLOViolation` - < 95% success rate for 1h (WARNING)
   - `ErrorBudgetDepleted` - < 10% error budget remaining (EMERGENCY)

6. **storage-quotas** - Storage monitoring
   - `AgentInboxQuotaWarning` - > 80% of 50GB quota for 30m (WARNING)
   - `AgentInboxQuotaExceeded` - > 95% of 50GB quota for 10m (CRITICAL)
   - `HighDiskIOWait` - > 80% I/O wait for 10m (WARNING)

## SLI/SLO Definitions

### Service Level Indicators (SLIs)

| SLI | Query | Target |
|-----|-------|--------|
| **Agent Availability** | `(avg_over_time(agentic_agents_by_status{status="ready"}[5m]) / scalar(agentic_agents_connected)) * 100` | 99.0% |
| **Command Success Rate** | `(rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m])) * 100` | 99.0% |
| **Task Success Rate** | `(rate(agentic_task_outcomes_total{outcome="success"}[1h]) / (rate(agentic_task_outcomes_total{outcome="success"}[1h]) + rate(agentic_task_outcomes_total{outcome="failure"}[1h]))) * 100` | 95.0% |

### Error Budget

Error budget is calculated as:
```
error_budget_remaining = 1 - (actual_error_rate / allowed_error_rate)
```

For a 99% SLO, allowed error rate is 1%. If actual error rate is 0.5%, error budget remaining is 50%.

**Error Budget Thresholds:**
- `> 50%` - Normal operations
- `25-50%` - Freeze risky deployments
- `10-25%` - Block non-essential changes (CRITICAL alert)
- `< 10%` - Emergency escalation, rollback last deployment (EMERGENCY alert)

## Grafana Dashboards

### Pre-built Dashboards

1. **Agent Fleet Overview** (Import ID: TBD)
   - Total agents, ready/busy status
   - Agent availability timeline
   - Fleet CPU/memory usage
   - Per-agent resource heatmap
   - Command rate and latency
   - Agent list table

2. **Task Orchestration** (Import ID: TBD)
   - Task states (pending, running, completed, failed)
   - Task lifecycle timeline
   - Task success/failure rates
   - Task duration analysis
   - Active tasks table

3. **Storage & Quotas** (Import ID: TBD)
   - Global storage summary
   - Per-agent inbox usage bar gauge
   - Storage growth trends
   - Disk I/O performance

4. **SLO Compliance** (Import ID: TBD)
   - SLO health summary with traffic lights
   - Error budget burn rate
   - SLI trends (7-day view)
   - SLO violation log

### Accessing Grafana

```bash
# Default credentials: admin/admin
# Access: http://localhost:3000

# Add Prometheus data source:
# Configuration → Data Sources → Add data source → Prometheus
# URL: http://localhost:9090
# Save & Test
```

## Useful PromQL Queries

### Agent Health

```promql
# Agent CPU usage by agent
100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle",job="agent-vms"}[5m])) * 100)

# Agent memory usage by agent
(1 - (node_memory_MemAvailable_bytes{job="agent-vms"} / node_memory_MemTotal_bytes{job="agent-vms"})) * 100

# Agent uptime (hours)
(time() - node_boot_time_seconds{job="agent-vms"}) / 3600
```

### Command Execution

```promql
# Command success rate (rolling 5m)
rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m])

# Command latency P95
histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m]))

# Commands per second
rate(agentic_commands_total[1m])
```

### Storage

```promql
# Agents over 80% inbox quota
(agentic_agentshare_inbox_bytes / (50 * 1024 * 1024 * 1024)) > 0.80

# Total inbox storage used (GB)
sum(agentic_agentshare_inbox_bytes) / (1024 * 1024 * 1024)
```

## Troubleshooting

### Prometheus Not Scraping Targets

```bash
# Check Prometheus logs
sudo journalctl -u prometheus -f

# Verify target reachability
curl http://192.168.122.201:9100/metrics

# Check Prometheus configuration
promtool check config /etc/prometheus/prometheus.yml
```

### Alerts Not Firing

```bash
# Check alert rules syntax
promtool check rules /etc/prometheus/rules/agentic-sandbox.yml

# View active alerts
curl http://localhost:9090/api/v1/alerts | jq '.data.alerts'

# Check Alertmanager status
curl http://localhost:9093/api/v1/status | jq '.'
```

### Node Exporter Not Running on Agent VM

```bash
# SSH to agent VM
ssh agent@192.168.122.201

# Check service status
sudo systemctl status prometheus-node-exporter

# View logs
sudo journalctl -u prometheus-node-exporter -f

# Restart service
sudo systemctl restart prometheus-node-exporter
```

### Custom Agent Metrics Not Appearing

```bash
# Check if textfile collector path exists
ls -la /var/lib/prometheus/node-exporter/

# View metrics file
cat /var/lib/prometheus/node-exporter/agent.prom

# Check node_exporter textfile collector
curl http://192.168.122.201:9100/metrics | grep agentic_agent
```

## Runbooks

Runbooks for each alert are linked in the alert annotations. Host them at:
- `https://docs.example.com/runbooks/agent-high-cpu`
- `https://docs.example.com/runbooks/agent-disconnected`
- etc.

See `docs/OBSERVABILITY_DESIGN.md` Appendix C for runbook templates.

## Production Hardening Checklist

- [ ] Prometheus retention configured (90 days recommended)
- [ ] Prometheus backup/restore tested
- [ ] Alertmanager silencing procedure documented
- [ ] Slack webhook URL configured
- [ ] PagerDuty service key configured
- [ ] On-call rotation established
- [ ] Alert runbooks created and reviewed
- [ ] Grafana dashboards imported and tested
- [ ] SLO targets reviewed with stakeholders
- [ ] Error budget policy approved
- [ ] Capacity planning baselines established
- [ ] ORR (Operational Readiness Review) completed

## References

- **Design Document:** `docs/OBSERVABILITY_DESIGN.md`
- **Management Metrics:** `management/src/telemetry/metrics.rs`
- **Agent Metrics Exporter:** `agent-rs/src/metrics_exporter.rs`
- **Prometheus Docs:** https://prometheus.io/docs/
- **Node Exporter:** https://github.com/prometheus/node_exporter
- **Grafana Docs:** https://grafana.com/docs/

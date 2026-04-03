# Monitoring Guide

Comprehensive monitoring and observability for the agentic-sandbox VM orchestration platform.

**Status:** Production-ready
**Last Updated:** 2026-02-01
**Audience:** Operators, DevOps engineers, SREs

---

## Quick Start

For operators who want to get monitoring up and running quickly.

### Prerequisites

```bash
# Ubuntu 24.04 LTS or later
sudo apt update
sudo apt install -y prometheus prometheus-alertmanager grafana

# Start services
sudo systemctl enable --now prometheus alertmanager grafana-server
```

### Deploy Configuration

```bash
# Navigate to repository root
cd /path/to/agentic-sandbox

# Deploy Prometheus configuration
sudo cp scripts/prometheus/prometheus.yml /etc/prometheus/prometheus.yml
sudo cp scripts/prometheus/rules/agentic-sandbox.yml /etc/prometheus/rules/agentic-sandbox.yml
sudo chown -R prometheus:prometheus /etc/prometheus

# Deploy Alertmanager configuration
sudo cp scripts/prometheus/alertmanager.yml /etc/alertmanager/alertmanager.yml
sudo chown prometheus:prometheus /etc/alertmanager/alertmanager.yml

# Edit Alertmanager to add your notification channels
sudo nano /etc/alertmanager/alertmanager.yml
# Replace YOUR/WEBHOOK/URL with Slack webhook URL

# Restart services
sudo systemctl restart prometheus alertmanager
```

### Verify Deployment

```bash
# Check Prometheus targets
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | {job, health}'

# Check alerts are loading
curl http://localhost:9090/api/v1/alerts | jq '.data.alerts | length'

# Access dashboards
echo "Prometheus: http://localhost:9090"
echo "Grafana: http://localhost:3000 (admin/admin)"
echo "Alertmanager: http://localhost:9093"
```

### Import Grafana Dashboard

1. Access Grafana at http://localhost:3000 (default credentials: admin/admin)
2. Navigate to **Configuration** → **Data Sources** → **Add data source**
3. Select **Prometheus**
4. Set URL to `http://localhost:9090`
5. Click **Save & Test**
6. Navigate to **Dashboards** → **Import**
7. Upload `scripts/prometheus/agentic-sandbox.json`
8. Select Prometheus data source
9. Click **Import**

---

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                        Host System                                │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              Management Server (Rust)                    │   │
│  │  • Exports /metrics endpoint (port 8122)                 │   │
│  │  • Atomic counters (lock-free)                           │   │
│  │  • Histogram buckets for latency                         │   │
│  │  • Per-agent labeled metrics                             │   │
│  └────────────┬─────────────────────────────────────────────┘   │
│               │                                                   │
│               │ scrape every 15s                                  │
│               │                                                   │
│  ┌────────────▼─────────────────────────────────────────────┐   │
│  │            Prometheus (localhost:9090)                   │   │
│  │  • Stores time-series metrics (90-day retention)         │   │
│  │  • Evaluates alert rules (every 30-60s)                  │   │
│  │  • Records SLI/SLO metrics                               │   │
│  └────────────┬─────────────────────────────────────────────┘   │
│               │                                                   │
│               │ send alerts                                       │
│               │                                                   │
│  ┌────────────▼─────────────────────────────────────────────┐   │
│  │         Alertmanager (localhost:9093)                    │   │
│  │  • Routes alerts by severity                             │   │
│  │  • Inhibits redundant alerts                             │   │
│  │  • Sends to Slack/PagerDuty/Email                        │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              Grafana (localhost:3000)                    │   │
│  │  • Queries Prometheus for visualization                  │   │
│  │  • Pre-built dashboards (import JSON)                    │   │
│  │  • SLO compliance tracking                               │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              Agent VMs (192.168.122.0/24)                │   │
│  │  • node_exporter on port 9100                            │   │
│  │  • System metrics (CPU, memory, disk, network)           │   │
│  │  • Custom metrics via textfile collector                 │   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

---

## Configuration Reference

### Environment Variables

The management server supports extensive telemetry configuration via environment variables.

#### Metrics Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `METRICS_ENABLED` | boolean | `true` | Enable/disable Prometheus metrics endpoint |

**Example:**
```bash
# Disable metrics (not recommended for production)
export METRICS_ENABLED=false
./management/target/release/agentic-management
```

#### Logging Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LOG_LEVEL` | string | `info` | Log verbosity: `trace`, `debug`, `info`, `warn`, `error` |
| `LOG_FORMAT` | string | `pretty` | Output format: `pretty`, `json`, `compact` |
| `LOG_FILE` | path | (none) | Optional file path for log output |
| `LOG_FILE_ROTATION` | string | `daily` | Rotation policy: `hourly`, `daily`, `never` |
| `LOG_FILE_RETENTION_DAYS` | integer | `7` | Days to retain rotated log files |

**Production Example:**
```bash
export LOG_LEVEL=info
export LOG_FORMAT=json
export LOG_FILE=/var/log/agentic-management/server.log
export LOG_FILE_ROTATION=daily
export LOG_FILE_RETENTION_DAYS=30

./management/target/release/agentic-management
```

**Development Example:**
```bash
export LOG_LEVEL=debug
export LOG_FORMAT=pretty

./management/dev.sh
```

#### Log Format Examples

**Pretty Format (human-readable, colored):**
```
2026-02-01T10:30:45.123Z  INFO agentic_management::grpc: agent connected agent_id="agent-01"
2026-02-01T10:30:46.234Z  INFO agentic_management::dispatch: command dispatched command_id="cmd-abc123"
2026-02-01T10:30:47.345Z  WARN agentic_management::orchestrator: task timeout task_id="task-xyz789"
```

**JSON Format (machine-readable, for log aggregation):**
```json
{"timestamp":"2026-02-01T10:30:45.123Z","level":"INFO","target":"agentic_management::grpc","fields":{"message":"agent connected","agent_id":"agent-01"}}
{"timestamp":"2026-02-01T10:30:46.234Z","level":"INFO","target":"agentic_management::dispatch","fields":{"message":"command dispatched","command_id":"cmd-abc123"}}
{"timestamp":"2026-02-01T10:30:47.345Z","level":"WARN","target":"agentic_management::orchestrator","fields":{"message":"task timeout","task_id":"task-xyz789"}}
```

**Compact Format (minimal, single-line):**
```
10:30:45 INFO agent connected agent_id="agent-01"
10:30:46 INFO command dispatched command_id="cmd-abc123"
10:30:47 WARN task timeout task_id="task-xyz789"
```

---

## Prometheus Setup

### Scrape Configuration

The management server exposes Prometheus metrics at `http://localhost:8122/metrics`.

Container runtime signals are exposed by the management server when Docker monitoring is enabled:
- `agentic_containers_by_status{status="running"}` / `agentic_containers_by_status{status="stopped"}`

**Recommended scrape interval:** 15 seconds

**Example prometheus.yml snippet:**
```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

scrape_configs:
  # Management server metrics
  - job_name: 'management-server'
    static_configs:
      - targets: ['localhost:8122']
        labels:
          service: 'agentic-sandbox'
          component: 'management'

  # Agent VM system metrics (node_exporter)
  - job_name: 'agent-vms'
    static_configs:
      - targets:
          - '192.168.122.201:9100'  # agent-01
          - '192.168.122.202:9100'  # agent-02
          - '192.168.122.203:9100'  # agent-03
        labels:
          service: 'agentic-sandbox'
          component: 'agent'

  # Dynamic agent discovery (optional)
  - job_name: 'agent-vms-dynamic'
    file_sd_configs:
      - files:
          - '/etc/prometheus/targets/agents.json'
        refresh_interval: 30s
```

### Alert Rules

Alert rules are defined in `scripts/prometheus/rules/agentic-sandbox.yml` and organized into six groups:

1. **agent-health** - Agent VM resource monitoring
2. **command-execution** - Command processing monitoring
3. **task-orchestration** - Task lifecycle monitoring
4. **management-server** - Server health monitoring
5. **slo-violations** - SLO compliance monitoring
6. **storage-quotas** - Storage monitoring

**Total alert rules:** 18 (across all severity levels)

**Alert rule deployment:**
```bash
sudo cp scripts/prometheus/rules/agentic-sandbox.yml /etc/prometheus/rules/
sudo chown prometheus:prometheus /etc/prometheus/rules/agentic-sandbox.yml
sudo systemctl reload prometheus
```

**Verify alert rules loaded:**
```bash
curl http://localhost:9090/api/v1/rules | jq '.data.groups[] | {name, file}'
```

### Alert Rule Examples

#### Critical Alert: Agent Disconnected

```yaml
- alert: AgentDisconnected
  expr: |
    up{job="agent-vms"} == 0
  for: 2m
  labels:
    severity: critical
    component: agent
  annotations:
    summary: "Agent {{ $labels.agent_id }} disconnected"
    description: "Agent has been unreachable for 2 minutes"
```

**Triggered when:** node_exporter on agent VM becomes unreachable for 2 minutes
**Response:** Page on-call engineer via PagerDuty
**Runbook:** https://docs.example.com/runbooks/agent-disconnected

#### Warning Alert: High Command Failure Rate

```yaml
- alert: HighCommandFailureRate
  expr: |
    (rate(agentic_commands_by_result{result="failed"}[5m]) / rate(agentic_commands_total[5m])) > 0.05
  for: 10m
  labels:
    severity: warning
    component: management
  annotations:
    summary: "High command failure rate"
    description: "{{ $value | humanizePercentage }} of commands are failing (> 5% threshold)"
```

**Triggered when:** More than 5% of commands fail over a 10-minute period
**Response:** Best-effort during business hours
**Runbook:** https://docs.example.com/runbooks/high-command-failure-rate

#### Emergency Alert: Error Budget Depleted

```yaml
- alert: ErrorBudgetDepleted
  expr: |
    (
      (rate(agentic_commands_by_result{result="failed"}[24h]) +
       rate(agentic_task_outcomes_total{outcome="failure"}[24h])) /
      (rate(agentic_commands_total[24h]) + rate(agentic_tasks_total[24h]))
    ) > 0.009
  for: 10m
  labels:
    severity: emergency
    component: slo
  annotations:
    summary: "Error budget critically depleted"
    description: "Error rate is {{ $value | humanizePercentage }} (> 0.9%, leaving < 10% error budget)"
```

**Triggered when:** Combined error rate exceeds 0.9%, leaving less than 10% of error budget
**Response:** Immediate escalation to incident commander
**Runbook:** https://docs.example.com/runbooks/error-budget-depleted

---

## Grafana Setup

### Data Source Configuration

1. Navigate to **Configuration** → **Data Sources**
2. Click **Add data source**
3. Select **Prometheus**
4. Configure:
   - **Name:** Prometheus
   - **URL:** http://localhost:9090
   - **Access:** Server (default)
   - **Scrape interval:** 15s
5. Click **Save & Test**

### Dashboard Import

The pre-built Grafana dashboard is located at `scripts/prometheus/agentic-sandbox.json`.

**Import steps:**
1. Navigate to **Dashboards** → **Import**
2. Upload `scripts/prometheus/agentic-sandbox.json`
3. Select **Prometheus** as the data source
4. Click **Import**

**Dashboard UID:** `agentic-sandbox-overview`
**Refresh interval:** 30 seconds (auto-refresh)

### Dashboard Panels

The dashboard includes the following panels:

#### 1. Fleet Status (Stat Panels)
- Total agents connected
- Agents ready
- Agents busy
- Current tasks running

#### 2. Agent Availability (Time Series)
- Agent count over time (stacked by status)
- 99% SLO threshold line
- 7-day view

#### 3. Command Throughput (Time Series)
- Commands per second
- Success vs. failure rate
- 5-minute rate

#### 4. Command Latency (Time Series)
- P50, P95, P99 latency percentiles
- Histogram quantiles
- 5-second threshold line (warning)

#### 5. Task States (Pie Chart)
- Distribution: pending, running, completed, failed
- Live counts

#### 6. Error Rate (Time Series)
- Command failure rate percentage
- Task failure rate percentage
- 5% threshold line (warning)

#### 7. gRPC Request Rate (Time Series)
- Requests per second by method (Connect, Dispatch)
- 1-minute rate

#### 8. WebSocket Connections (Time Series)
- Current active connections
- Total connections (counter)

#### 9. Agent Resource Usage (Heatmap)
- CPU usage per agent
- Memory usage per agent
- Disk usage per agent

#### 10. Storage Quotas (Bar Gauge)
- Per-agent inbox usage (GB)
- 50GB quota threshold (red at 80%)

### Variable Configuration

Dashboard supports the following variables for filtering:

- **$agent_id** - Filter by specific agent (multi-select)
- **$time_range** - Customize time range (5m, 15m, 1h, 6h, 24h, 7d)

---

## OpenTelemetry Setup (Optional)

The management server includes OpenTelemetry support for distributed tracing.

### Configuration

```bash
# Enable OpenTelemetry (currently disabled by default)
export OTEL_ENABLED=true
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
export OTEL_SERVICE_NAME=agentic-management

./management/target/release/agentic-management
```

### Tempo Backend

**Example docker-compose.yml for Tempo:**
```yaml
version: '3.8'
services:
  tempo:
    image: grafana/tempo:2.3.0
    command: ["-config.file=/etc/tempo.yaml"]
    volumes:
      - ./tempo.yaml:/etc/tempo.yaml
      - tempo-data:/var/tempo
    ports:
      - "4317:4317"  # OTLP gRPC
      - "3200:3200"  # Tempo HTTP

volumes:
  tempo-data:
```

### Jaeger Backend

**Alternative using Jaeger:**
```bash
docker run -d --name jaeger \
  -e COLLECTOR_OTLP_ENABLED=true \
  -p 4317:4317 \
  -p 16686:16686 \
  jaegertracing/all-in-one:1.50
```

Access Jaeger UI at http://localhost:16686

### Trace Correlation

When OpenTelemetry is enabled, all logs include trace context:

```json
{
  "timestamp": "2026-02-01T10:30:45.123Z",
  "level": "INFO",
  "target": "agentic_management::dispatch",
  "trace_id": "a1b2c3d4e5f6g7h8i9j0",
  "span_id": "1a2b3c4d5e6f7g8h",
  "fields": {
    "message": "command dispatched",
    "command_id": "cmd-abc123"
  }
}
```

Query logs by trace_id in Loki or correlate traces in Grafana Explore.

---

## Alerting

### Severity Levels

| Severity | Count | Response Time | Notification Channel | Example |
|----------|-------|---------------|---------------------|---------|
| **WARNING** | 10 | Best-effort, business hours | Slack #alerts | Agent CPU > 80% for 10m |
| **CRITICAL** | 7 | < 30 minutes | PagerDuty + Slack | Management server down |
| **EMERGENCY** | 1 | Immediate | PagerDuty + Slack #incidents + SMS | Error budget < 10% |

### Alert Groups

#### 1. Agent Health (4 alerts)

| Alert | Severity | Threshold | Duration |
|-------|----------|-----------|----------|
| AgentHighCPU | WARNING | > 80% | 10m |
| AgentHighMemory | WARNING | > 85% | 10m |
| AgentDiskFull | CRITICAL | > 90% | 5m |
| AgentDisconnected | CRITICAL | Unreachable | 2m |

#### 2. Command Execution (3 alerts)

| Alert | Severity | Threshold | Duration |
|-------|----------|-----------|----------|
| HighCommandFailureRate | WARNING | > 5% | 10m |
| CommandExecutionStalled | CRITICAL | 0 commands with running tasks | 5m |
| SlowCommandExecution | WARNING | P95 > 5s | 10m |

#### 3. Task Orchestration (3 alerts)

| Alert | Severity | Threshold | Duration |
|-------|----------|-----------|----------|
| HighTaskFailureRate | WARNING | > 10% | 30m |
| TaskQueueBacklog | CRITICAL | > 10 pending | 15m |
| HighTaskTimeoutRate | WARNING | > 5% | 30m |

#### 4. Management Server (3 alerts)

| Alert | Severity | Threshold | Duration |
|-------|----------|-----------|----------|
| ManagementServerDown | CRITICAL | Unreachable | 2m |
| HighGrpcErrorRate | WARNING | > 5% | 10m |
| HighWebSocketChurn | WARNING | > 10 conn/s | 15m |

#### 5. SLO Violations (4 alerts)

| Alert | Severity | Threshold | Duration |
|-------|----------|-----------|----------|
| AgentAvailabilitySLOViolation | CRITICAL | < 99% | 30m |
| CommandSuccessRateSLOViolation | CRITICAL | < 99% | 30m |
| TaskSuccessRateSLOViolation | WARNING | < 95% | 1h |
| ErrorBudgetDepleted | EMERGENCY | < 10% remaining | 10m |

#### 6. Storage Quotas (3 alerts)

| Alert | Severity | Threshold | Duration |
|-------|----------|-----------|----------|
| AgentInboxQuotaWarning | WARNING | > 80% of 50GB | 30m |
| AgentInboxQuotaExceeded | CRITICAL | > 95% of 50GB | 10m |
| HighDiskIOWait | WARNING | > 80% | 10m |

### Notification Routing

**Alertmanager routing configuration:**

```yaml
route:
  group_by: ['alertname', 'severity', 'component']
  group_wait: 30s
  group_interval: 5m
  repeat_interval: 4h
  receiver: 'slack-alerts'

  routes:
    # CRITICAL and EMERGENCY to PagerDuty
    - match_re:
        severity: ^(critical|emergency)$
      receiver: 'pagerduty'
      continue: true  # Also send to Slack

    # EMERGENCY to incident channel
    - match:
        severity: emergency
      receiver: 'slack-incidents'

    # All alerts to general channel
    - receiver: 'slack-alerts'

receivers:
  - name: 'slack-alerts'
    slack_configs:
      - api_url: 'https://hooks.slack.com/services/YOUR/WEBHOOK/URL'
        channel: '#agentic-sandbox-alerts'
        title: '{{ .GroupLabels.alertname }}'
        text: '{{ range .Alerts }}{{ .Annotations.summary }}\n{{ end }}'

  - name: 'slack-incidents'
    slack_configs:
      - api_url: 'https://hooks.slack.com/services/YOUR/WEBHOOK/URL'
        channel: '#agentic-sandbox-incidents'
        title: 'EMERGENCY: {{ .GroupLabels.alertname }}'
        text: '{{ range .Alerts }}{{ .Annotations.description }}\n{{ end }}'

  - name: 'pagerduty'
    pagerduty_configs:
      - service_key: 'YOUR_PAGERDUTY_SERVICE_KEY'
        description: '{{ .GroupLabels.alertname }}'
```

### Integration with PagerDuty

1. Create a new service in PagerDuty
2. Select **Events API v2** integration
3. Copy the integration key
4. Add to `alertmanager.yml` under `pagerduty_configs.service_key`
5. Restart Alertmanager

### Integration with Slack

1. Create a new Slack app at https://api.slack.com/apps
2. Enable **Incoming Webhooks**
3. Add webhook to workspace (select channels: #agentic-sandbox-alerts, #agentic-sandbox-incidents)
4. Copy webhook URLs
5. Add to `alertmanager.yml` under `slack_configs.api_url`
6. Restart Alertmanager

---

## Useful PromQL Queries

### Agent Health

```promql
# Agent CPU usage by agent (percentage)
100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle",job="agent-vms"}[5m])) * 100)

# Agent memory usage by agent (percentage)
(1 - (node_memory_MemAvailable_bytes{job="agent-vms"} / node_memory_MemTotal_bytes{job="agent-vms"})) * 100

# Agent disk usage by agent (percentage)
(1 - (node_filesystem_avail_bytes{mountpoint="/",job="agent-vms"} / node_filesystem_size_bytes{mountpoint="/",job="agent-vms"})) * 100

# Agent uptime (hours)
(time() - node_boot_time_seconds{job="agent-vms"}) / 3600

# Agents that are unreachable
up{job="agent-vms"} == 0
```

### Command Execution

```promql
# Command success rate (percentage, 5m window)
(rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m])) * 100

# Command failure rate (percentage, 5m window)
(rate(agentic_commands_by_result{result="failed"}[5m]) / rate(agentic_commands_total[5m])) * 100

# Commands per second (1m rate)
rate(agentic_commands_total[1m])

# Command latency P50 (median)
histogram_quantile(0.50, rate(agentic_command_latency_seconds_bucket[5m]))

# Command latency P95
histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m]))

# Command latency P99
histogram_quantile(0.99, rate(agentic_command_latency_seconds_bucket[5m]))
```

### Task Orchestration

```promql
# Tasks pending (current count)
agentic_tasks_by_state{state="pending"}

# Tasks running (current count)
agentic_tasks_by_state{state="running"}

# Task success rate (percentage, 1h window)
(rate(agentic_task_outcomes_total{outcome="success"}[1h]) /
 (rate(agentic_task_outcomes_total{outcome="success"}[1h]) +
  rate(agentic_task_outcomes_total{outcome="failure"}[1h]))) * 100

# Task failure rate (percentage, 1h window)
(rate(agentic_task_outcomes_total{outcome="failure"}[1h]) /
 (rate(agentic_task_outcomes_total{outcome="success"}[1h]) +
  rate(agentic_task_outcomes_total{outcome="failure"}[1h]))) * 100

# Task timeout rate (percentage, 1h window)
(rate(agentic_task_outcomes_total{outcome="timeout"}[1h]) / rate(agentic_tasks_total[1h])) * 100
```

### Storage

```promql
# Agents over 80% inbox quota
(agentic_agentshare_inbox_bytes / (50 * 1024 * 1024 * 1024)) > 0.80

# Total inbox storage used (GB)
sum(agentic_agentshare_inbox_bytes) / (1024 * 1024 * 1024)

# Per-agent inbox usage (GB)
agentic_agentshare_inbox_bytes / (1024 * 1024 * 1024)

# Disk I/O wait time (percentage)
rate(node_disk_io_time_seconds_total{job="agent-vms"}[5m]) * 100
```

### SLO Compliance

```promql
# Agent availability SLO (target: 99%)
(avg_over_time(agentic_agents_by_status{status="ready"}[5m]) /
 scalar(agentic_agents_connected)) * 100

# Command success rate SLO (target: 99%)
(rate(agentic_commands_by_result{result="success"}[1h]) /
 rate(agentic_commands_total[1h])) * 100

# Task success rate SLO (target: 95%)
(rate(agentic_task_outcomes_total{outcome="success"}[1h]) /
 (rate(agentic_task_outcomes_total{outcome="success"}[1h]) +
  rate(agentic_task_outcomes_total{outcome="failure"}[1h]))) * 100

# Combined error budget remaining (percentage)
(1 - (
  (rate(agentic_commands_by_result{result="failed"}[24h]) +
   rate(agentic_task_outcomes_total{outcome="failure"}[24h])) /
  (rate(agentic_commands_total[24h]) + rate(agentic_tasks_total[24h]))
) / 0.01) * 100
```

---

## Troubleshooting

### Metrics Not Appearing in Prometheus

**Symptom:** No metrics visible for management server or agent VMs

**Check 1: Verify metrics endpoint is accessible**
```bash
curl http://localhost:8122/metrics
```

Expected: Prometheus text format output with metric names starting with `agentic_`

**Check 2: Verify Prometheus scrape configuration**
```bash
promtool check config /etc/prometheus/prometheus.yml
```

Expected: `SUCCESS: 0 rule files found`

**Check 3: Verify Prometheus is scraping target**
```bash
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="management-server")'
```

Expected: `"health": "up"` and `"lastError": ""`

**Check 4: Check Prometheus logs**
```bash
sudo journalctl -u prometheus -n 50 --no-pager
```

Look for: Scrape errors, configuration errors, target discovery issues

### Alerts Not Firing

**Symptom:** Expected alerts not triggering despite conditions being met

**Check 1: Verify alert rules syntax**
```bash
promtool check rules /etc/prometheus/rules/agentic-sandbox.yml
```

Expected: `SUCCESS: X rules found`

**Check 2: View active alerts in Prometheus**
```bash
curl http://localhost:9090/api/v1/alerts | jq '.data.alerts[] | {alertname: .labels.alertname, state: .state}'
```

Expected: Alert should appear with `"state": "firing"` when condition is met

**Check 3: Check Alertmanager status**
```bash
curl http://localhost:9093/api/v1/status | jq '.'
```

Expected: `"uptime"` field present, `"versionInfo"` matches installed version

**Check 4: Check Alertmanager logs**
```bash
sudo journalctl -u alertmanager -n 50 --no-pager
```

Look for: Routing errors, notification failures

### Grafana Dashboard Not Loading Data

**Symptom:** Dashboard panels show "No Data" or errors

**Check 1: Verify Prometheus data source**
- Navigate to **Configuration** → **Data Sources** → **Prometheus**
- Click **Save & Test**
- Should show green "Data source is working" message

**Check 2: Verify PromQL queries in Explore**
- Navigate to **Explore**
- Select **Prometheus** data source
- Run test query: `agentic_agents_connected`
- Should return current agent count

**Check 3: Check dashboard time range**
- Ensure time range is set to "Last 1 hour" or similar
- Check that data exists for selected time range

**Check 4: Check browser console for errors**
- Open browser DevTools (F12)
- Look for JavaScript errors or network errors

### Agent VMs Not Reporting Metrics

**Symptom:** node_exporter metrics missing for specific agent VMs

**Check 1: Verify node_exporter is running**
```bash
ssh agent@192.168.122.201
sudo systemctl status prometheus-node-exporter
```

Expected: `active (running)`

**Check 2: Verify node_exporter endpoint**
```bash
curl http://192.168.122.201:9100/metrics | head -20
```

Expected: Prometheus text format output with metrics like `node_cpu_seconds_total`

**Check 3: Check firewall rules**
```bash
ssh agent@192.168.122.201
sudo ufw status
```

Expected: Port 9100 allowed from host IP

**Check 4: Verify network connectivity**
```bash
ping -c 3 192.168.122.201
nc -zv 192.168.122.201 9100
```

Expected: Ping successful, port 9100 open

### High Memory Usage in Prometheus

**Symptom:** Prometheus consuming excessive memory

**Check 1: Review retention settings**
```bash
grep -i retention /etc/prometheus/prometheus.yml
```

Recommended: 90 days (`--storage.tsdb.retention.time=90d`)

**Check 2: Check TSDB size**
```bash
du -sh /var/lib/prometheus/metrics2
```

Expected: Roughly 50MB per day per agent (1.5GB per agent for 30 days)

**Check 3: Reduce scrape interval if needed**
```yaml
global:
  scrape_interval: 30s  # Increase from 15s to reduce load
```

**Check 4: Consider remote write for long-term storage**
```yaml
remote_write:
  - url: http://remote-prometheus:9090/api/v1/write
```

---

## Production Checklist

### Pre-Deployment

- [ ] Prometheus installed and configured
- [ ] Grafana installed and configured
- [ ] Alertmanager installed and configured
- [ ] Slack webhook URL configured
- [ ] PagerDuty integration configured
- [ ] Alert rules deployed
- [ ] Dashboard imported
- [ ] Data source tested
- [ ] Firewall rules configured (ports 9090, 9093, 3000)
- [ ] node_exporter installed on all agent VMs
- [ ] Retention policy configured (90 days)
- [ ] Backup strategy for Prometheus TSDB

### Post-Deployment

- [ ] All targets showing as "UP" in Prometheus
- [ ] At least one alert firing (test alert)
- [ ] Slack notifications received
- [ ] PagerDuty notifications received
- [ ] Dashboard loading data successfully
- [ ] All panels populated with metrics
- [ ] SLO compliance at target levels
- [ ] Error budget > 50%
- [ ] Runbooks created for critical alerts
- [ ] On-call rotation established
- [ ] Escalation policy documented
- [ ] ORR (Operational Readiness Review) completed

---

## References

- **Alert Rules:** `scripts/prometheus/rules/agentic-sandbox.yml`
- **Prometheus Config:** `scripts/prometheus/prometheus.yml`
- **Alertmanager Config:** `scripts/prometheus/alertmanager.yml`
- **Grafana Dashboard:** `scripts/prometheus/agentic-sandbox.json`
- **Metrics Implementation:** `management/src/telemetry/metrics.rs`
- **Logging Implementation:** `management/src/telemetry/logging.rs`
- **Observability Design:** `docs/OBSERVABILITY_DESIGN.md`
- **Quick Reference:** `docs/observability/QUICK_REFERENCE.md`
- **Prometheus Documentation:** https://prometheus.io/docs/
- **Grafana Documentation:** https://grafana.com/docs/
- **Node Exporter:** https://github.com/prometheus/node_exporter

---

**Last Updated:** 2026-02-01
**Maintained By:** Platform Team
**Status:** Production-ready

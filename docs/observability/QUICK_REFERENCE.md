# Observability Quick Reference

**Agentic Sandbox Monitoring & Alerting Cheat Sheet**

---

## URLs

| Service | URL | Credentials |
|---------|-----|-------------|
| Prometheus | http://localhost:9090 | None |
| Grafana | http://localhost:3000 | admin / (set during install) |
| Alertmanager | http://localhost:9093 | None |
| Management /metrics | http://localhost:8122/metrics | None |
| Agent VM metrics | http://192.168.122.201:9100/metrics | None |

---

## Key PromQL Queries

### Agent Health

```promql
# Agent availability percentage
(avg_over_time(agentic_agents_by_status{status="ready"}[5m]) / scalar(agentic_agents_connected)) * 100

# Agents with high CPU (> 80%)
100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100) > 80

# Agents with high memory (> 85%)
(1 - (node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes)) * 100 > 85

# Agent uptime in hours
(time() - node_boot_time_seconds) / 3600

# Agent disk usage percentage
(1 - (node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes{mountpoint="/"})) * 100
```

### Command Execution

```promql
# Command success rate (5 min rolling)
rate(agentic_commands_by_result{result="success"}[5m]) / rate(agentic_commands_total[5m])

# Command latency P95
histogram_quantile(0.95, rate(agentic_command_latency_seconds_bucket[5m]))

# Commands per second
rate(agentic_commands_total[1m])

# Failed commands in last hour
increase(agentic_commands_by_result{result="failed"}[1h])
```

### Task Orchestration

```promql
# Task success rate (1 hour rolling)
rate(agentic_tasks_by_state{state="completed"}[1h]) / rate(agentic_tasks_total[1h])

# Task queue depth
agentic_tasks_by_state{state="pending"}

# Running tasks count
agentic_tasks_by_state{state="running"}
```

### Storage

```promql
# Agents over 80% inbox quota
(agentic_agentshare_inbox_bytes / (50 * 1024^3)) > 0.80

# Total inbox storage used (GB)
sum(agentic_agentshare_inbox_bytes) / 1024^3

# Disk write rate (MB/s)
rate(node_disk_written_bytes_total[5m]) / 1024^2
```

---

## Key LogQL Queries

```logql
# All logs for agent-01
{agent_id="agent-01"}

# Failed commands
{agent_id=~".*"} |= "EXIT" != "EXIT 0"

# Logs from specific run
{run_id="run-20260131-143022"}

# Last hour of stderr logs
{job="agent-runs"} | json | stream="stderr" | __timestamp__ >= now() - 1h

# High-latency commands (> 5000ms)
{job="agent-runs"} | json | duration_ms > 5000
```

---

## Common Operations

### Check Prometheus Targets

```bash
# Via CLI
curl -s http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | {job: .labels.job, instance: .labels.instance, health: .health}'

# Via UI
http://localhost:9090/targets
```

### Reload Prometheus Config

```bash
# Method 1: Signal
sudo killall -HUP prometheus

# Method 2: Systemd
sudo systemctl reload prometheus

# Method 3: API (if --web.enable-lifecycle)
curl -X POST http://localhost:9090/-/reload
```

### Query Metrics from CLI

```bash
# Instant query
curl -s 'http://localhost:9090/api/v1/query?query=up' | jq .

# Range query (last hour)
curl -s 'http://localhost:9090/api/v1/query_range?query=up&start='$(date -d '1 hour ago' +%s)'&end='$(date +%s)'&step=60' | jq .
```

### Check Alertmanager Alerts

```bash
# Active alerts
curl -s http://localhost:9093/api/v2/alerts | jq '.[] | {name: .labels.alertname, state: .status.state, since: .startsAt}'

# Silence an alert (5 hours)
amtool silence add --alertmanager.url=http://localhost:9093 \
  alertname=AgentHighCPU agent_id=agent-01 \
  --duration=5h --comment="Planned maintenance"
```

### Query Logs from CLI

```bash
# Query Loki
curl -s 'http://localhost:3100/loki/api/v1/query_range?query={agent_id="agent-01"}&limit=10' | jq .

# Stream logs (like tail -f)
logcli query --addr=http://localhost:3100 '{agent_id="agent-01"}' --tail
```

### Backup Prometheus Data

```bash
# Create snapshot (requires --web.enable-admin-api)
curl -X POST http://localhost:9090/api/v1/admin/tsdb/snapshot
# Snapshot saved to /var/lib/prometheus/snapshots/

# Manual backup (stop Prometheus first)
sudo systemctl stop prometheus
sudo tar -czf /backup/prometheus-$(date +%Y%m%d).tar.gz /var/lib/prometheus/data
sudo systemctl start prometheus
```

---

## Alert Severity Levels

| Severity | Color | Response Time | Notification |
|----------|-------|--------------|--------------|
| **WARNING** | Yellow | Best-effort, business hours | Slack only |
| **CRITICAL** | Orange | < 30 minutes | PagerDuty |
| **EMERGENCY** | Red | Immediate | PagerDuty + Slack + SMS |

---

## SLO Targets

| SLO | Target | Measurement Window |
|-----|--------|--------------------|
| Agent Availability | 99.0% | Rolling 7 days |
| Command Success Rate | 99.0% | Rolling 24 hours |
| Task Success Rate | 95.0% | Rolling 7 days |
| Command Latency P95 | < 5s | Rolling 1 hour |

**Error Budget Formula:**
```
Error Budget Remaining (%) = 100 * (1 - (failures / total) / (1 - SLO_TARGET))
```

---

## Runbook Index

| Alert | Severity | Runbook |
|-------|----------|---------|
| AgentHighCPU | WARNING | `/docs/runbooks/agent-high-cpu.md` |
| AgentDown | CRITICAL | `/docs/runbooks/agent-down.md` |
| HighCommandFailureRate | WARNING | `/docs/runbooks/high-command-failure-rate.md` |
| TaskQueueBacklog | CRITICAL | `/docs/runbooks/task-backlog.md` |
| ManagementServerDown | CRITICAL | `/docs/runbooks/management-server-down.md` |
| ErrorBudgetDepleted | EMERGENCY | `/docs/runbooks/error-budget-emergency.md` |

---

## Troubleshooting

### Metrics Not Appearing

```bash
# 1. Check Prometheus targets
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.health != "up")'

# 2. Check agent node_exporter
ssh agent@192.168.122.201 'systemctl status prometheus-node-exporter'

# 3. Check management server metrics
curl http://localhost:8122/metrics | grep agentic_

# 4. Check Prometheus logs
journalctl -u prometheus -n 50 --no-pager
```

### Logs Not Appearing in Loki

```bash
# 1. Check Promtail status
systemctl status promtail
journalctl -u promtail -n 50 --no-pager

# 2. Check Loki ingestion
curl http://localhost:3100/metrics | grep loki_ingester_streams_created_total

# 3. Check log files exist
ls -lh /srv/agentshare/inbox/agent-01/runs/*/stdout.log

# 4. Test Promtail config
promtail --config.file=/etc/promtail/config.yml --dry-run
```

### Alerts Not Firing

```bash
# 1. Check alert rules loaded
curl http://localhost:9090/api/v1/rules | jq '.data.groups[].rules[] | {alert: .name, state: .state}'

# 2. Force evaluate rule
# (Query should return > 0 if condition is met)
curl -s 'http://localhost:9090/api/v1/query?query=100 - (avg by (agent_id) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100) > 80' | jq .

# 3. Check Alertmanager config
amtool config show --alertmanager.url=http://localhost:9093

# 4. Check Alertmanager routes
amtool config routes show --alertmanager.url=http://localhost:9093
```

### High Prometheus Memory Usage

```bash
# Check TSDB stats
curl http://localhost:9090/api/v1/status/tsdb | jq .

# Check retention settings
ps aux | grep prometheus | grep retention

# Compact TSDB manually
curl -X POST http://localhost:9090/api/v1/admin/tsdb/clean_tombstones

# Reduce retention if needed
sudo systemctl stop prometheus
# Edit /etc/default/prometheus: --storage.tsdb.retention.time=60d
sudo systemctl start prometheus
```

---

## Useful Commands

### Agent Operations

```bash
# SSH to agent
ssh agent@192.168.122.201

# Check agent service
systemctl status agentic-agent

# View agent logs
journalctl -u agentic-agent -f

# Check custom metrics
cat /var/lib/prometheus/node-exporter/agent.prom

# Restart agent client
sudo systemctl restart agentic-agent
```

### Management Server Operations

```bash
# Restart management server
cd /home/roctinam/dev/agentic-sandbox/management
./dev.sh restart

# View management logs
./dev.sh logs

# Check HTTP endpoints
curl http://localhost:8122/api/v1/agents
curl http://localhost:8122/metrics
curl http://localhost:8122/health
```

### Prometheus Operations

```bash
# Query from command line
promtool query instant http://localhost:9090 'up'
promtool query range http://localhost:9090 'up' --start='2024-01-01T00:00:00Z' --end='2024-01-01T01:00:00Z'

# Check rule syntax
promtool check rules /etc/prometheus/rules/*.yml

# Test alert expression
promtool test rules /path/to/test.yml
```

### Grafana Operations

```bash
# Restart Grafana
sudo systemctl restart grafana-server

# Import dashboard from JSON
curl -X POST http://admin:admin@localhost:3000/api/dashboards/db \
  -H "Content-Type: application/json" \
  -d @dashboard.json

# Export dashboard
curl -s http://admin:admin@localhost:3000/api/dashboards/uid/DASHBOARD_UID | jq .dashboard > export.json
```

---

## Performance Baselines

| Metric | Normal Range | Warning Threshold | Critical Threshold |
|--------|--------------|-------------------|-------------------|
| Agent CPU | 10-50% | 60-80% | > 80% |
| Agent Memory | 20-60% | 70-85% | > 85% |
| Agent Disk | 20-50% | 60-80% | > 80% |
| Command Latency P95 | < 2s | 2-5s | > 5s |
| Task Success Rate | > 98% | 95-98% | < 95% |
| Prometheus Query Latency | < 100ms | 100-500ms | > 500ms |

---

## Contact Information

| Role | Contact | Escalation |
|------|---------|------------|
| On-Call Engineer | PagerDuty rotation | Auto-escalate after 15 min |
| Platform Team Lead | [Name/Email] | Slack DM |
| DevOps Lead | [Name/Email] | Slack DM |
| VP Engineering | [Name/Email] | Email only |

**Slack Channels:**
- `#agentic-sandbox-alerts` - All warnings
- `#agentic-sandbox-incidents` - Critical/Emergency alerts
- `#agentic-sandbox-team` - General discussion

---

**Last Updated:** 2026-01-31
**Version:** 1.0

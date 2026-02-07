# Observability Architecture Diagram

## High-Level Overview

```
                           ┌─────────────────────────────────────┐
                           │      Grafana Dashboards             │
                           │  • Agent Fleet Overview             │
                           │  • Task Orchestration               │
                           │  • Storage & Quotas                 │
                           │  • SLO Compliance                   │
                           └──────────┬──────────────────────────┘
                                      │
                        ┌─────────────┴─────────────┐
                        │                           │
                 ┌──────▼──────┐            ┌──────▼──────┐
                 │  Prometheus │            │    Loki     │
                 │   (Metrics) │            │    (Logs)   │
                 └──────┬──────┘            └──────┬──────┘
                        │                           │
         ┌──────────────┼───────────────┬──────────┴────────┐
         │              │               │                   │
         │              │               │                   │
    ┌────▼────┐    ┌────▼────┐    ┌────▼────┐         ┌────▼────┐
    │ Mgmt    │    │ Host    │    │ Agent   │         │Promtail │
    │ Server  │    │ Node    │    │ VMs     │         │ (Ship)  │
    │ /metrics│    │Exporter │    │node_exp │         └─────────┘
    └─────────┘    └─────────┘    └─────────┘
         │              │               │
         │              │               │
         └──────────────┴───────────────┘
                        │
                 ┌──────▼──────┐
                 │Alertmanager │
                 └──────┬──────┘
                        │
         ┌──────────────┴───────────────┐
         │                              │
    ┌────▼────┐                    ┌────▼────┐
    │  Slack  │                    │PagerDuty│
    │Warnings │                    │Critical │
    └─────────┘                    └─────────┘
```

## Data Flow Diagram

```
┌───────────────────────────────────────────────────────────────────┐
│ COLLECTION LAYER                                                  │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌─────────────┐      ┌─────────────┐      ┌─────────────┐      │
│  │  Agent VM   │      │  Agent VM   │      │  Agent VM   │      │
│  │  agent-01   │      │  agent-02   │      │  agent-03   │      │
│  │             │      │             │      │             │      │
│  │ ┌─────────┐ │      │ ┌─────────┐ │      │ ┌─────────┐ │      │
│  │ │node_exp │ │      │ │node_exp │ │      │ │node_exp │ │      │
│  │ │ :9100   │ │      │ │ :9100   │ │      │ │ :9100   │ │      │
│  │ └────┬────┘ │      │ └────┬────┘ │      │ └────┬────┘ │      │
│  │      │      │      │      │      │      │      │      │      │
│  │ ┌────▼────┐ │      │ ┌────▼────┐ │      │ ┌────▼────┐ │      │
│  │ │Custom   │ │      │ │Custom   │ │      │ │Custom   │ │      │
│  │ │Metrics  │ │      │ │Metrics  │ │      │ │Metrics  │ │      │
│  │ │.prom    │ │      │ │.prom    │ │      │ │.prom    │ │      │
│  │ └─────────┘ │      │ └─────────┘ │      │ └─────────┘ │      │
│  │             │      │             │      │             │      │
│  │ ┌─────────┐ │      │ ┌─────────┐ │      │ ┌─────────┐ │      │
│  │ │Logs     │ │      │ │Logs     │ │      │ │Logs     │ │      │
│  │ │inbox/   │ │      │ │inbox/   │ │      │ │inbox/   │ │      │
│  │ │runs/*   │◄─┼──────┼─┤runs/*   │◄─┼──────┼─┤runs/*   │ │      │
│  │ └─────────┘ │ via  │ └─────────┘ │ via  │ └─────────┘ │      │
│  └─────────────┘virtiofs└──────────┘virtiofs└──────────┘      │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Management Server (Port 8122)                              │ │
│  │  /metrics → agentic_commands_*, agentic_tasks_*             │ │
│  │  gRPC heartbeats → aggregates agent metrics                 │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
                              │
                              │ HTTP Scrape (15s interval)
                              │
┌───────────────────────────────────────────────────────────────────┐
│ AGGREGATION LAYER                                                 │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Prometheus (Port 9090)                                     │ │
│  │  • Scrapes /metrics from all targets                        │ │
│  │  • Evaluates alert rules (30s/60s interval)                 │ │
│  │  • Stores TSDB with 90-day retention                        │ │
│  │  • PromQL query engine                                      │ │
│  └──────────────────┬──────────────────────────────────────────┘ │
│                     │                                             │
│                     │ Alerts                                      │
│                     ▼                                             │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Alertmanager (Port 9093)                                   │ │
│  │  • Groups alerts by severity/component                      │ │
│  │  • Inhibits duplicate alerts                                │ │
│  │  • Routes to receivers (Slack/PagerDuty)                    │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Loki (Port 3100)                                           │ │
│  │  • Receives logs from Promtail                              │ │
│  │  • Indexes by labels: agent_id, run_id, timestamp           │ │
│  │  • 30-day retention                                         │ │
│  │  • LogQL query engine                                       │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                     ▲                                             │
│                     │ Push logs                                   │
│  ┌──────────────────┴──────────────────────────────────────────┐ │
│  │  Promtail                                                   │ │
│  │  • Tails /srv/agentshare/inbox/*/runs/*/*.log               │ │
│  │  • Parses JSON logs                                         │ │
│  │  • Adds labels from file path regex                         │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
                              │
                              │ Query API
                              │
┌───────────────────────────────────────────────────────────────────┐
│ VISUALIZATION LAYER                                               │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Grafana (Port 3000)                                        │ │
│  │  ┌──────────────┬──────────────┬──────────────────────────┐ │ │
│  │  │ Agent Fleet  │ Task Orch    │ Storage & Quotas         │ │ │
│  │  │ Overview     │              │                          │ │ │
│  │  └──────────────┴──────────────┴──────────────────────────┘ │ │
│  │  ┌──────────────────────────────────────────────────────────┤ │
│  │  │ SLO Compliance Dashboard                               │ │ │
│  │  │ • Agent Availability: 99.4% ✅ (Target: 99%)            │ │ │
│  │  │ • Command Success Rate: 99.8% ✅                        │ │ │
│  │  │ • Error Budget: 60% remaining                          │ │ │
│  │  └──────────────────────────────────────────────────────────┘ │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
                              │
                              │ Notifications
                              │
┌───────────────────────────────────────────────────────────────────┐
│ NOTIFICATION LAYER                                                │
├───────────────────────────────────────────────────────────────────┤
│                                                                   │
│  ┌──────────────────┐          ┌──────────────────────┐          │
│  │  Slack           │          │  PagerDuty           │          │
│  │  #agentic-alerts │          │  On-call rotation    │          │
│  │                  │          │                      │          │
│  │  WARNING         │          │  CRITICAL/EMERGENCY  │          │
│  │  (informational) │          │  (pages engineer)    │          │
│  └──────────────────┘          └──────────────────────┘          │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
```

## Metrics Flow Detail

```
Agent VM
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ Agent Client Process (Rust)                            │ │
│  │                                                          │ │
│  │ ┌─────────────────┐        ┌──────────────────────────┐│ │
│  │ │Command Execution│        │AgentMetricsExporter      ││ │
│  │ │                 │        │                          ││ │
│  │ │ execute_cmd()   │───────▶│increment_commands()      ││ │
│  │ │ record_latency()│        │record_success(latency)   ││ │
│  │ └─────────────────┘        │record_failure(latency)   ││ │
│  │                            │                          ││ │
│  │                            │ Every 60s:               ││ │
│  │                            │ write_metrics()          ││ │
│  │                            └──────────┬───────────────┘│ │
│  └───────────────────────────────────────┼────────────────┘ │
│                                          │                  │
│                                          ▼                  │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ /var/lib/prometheus/node-exporter/agent.prom           │ │
│  │ # HELP agentic_agent_commands_total                    │ │
│  │ # TYPE agentic_agent_commands_total counter            │ │
│  │ agentic_agent_commands_total{agent_id="agent-01"} 142  │ │
│  │ agentic_agent_commands_success{agent_id="agent-01"} 138│ │
│  │ agentic_agent_commands_failed{agent_id="agent-01"} 4   │ │
│  └────────────────────────────┬───────────────────────────┘ │
│                               │                             │
│                               │ Read by node_exporter       │
│                               ▼                             │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ Node Exporter (Port 9100)                              │ │
│  │ • System metrics: CPU, memory, disk, network           │ │
│  │ • Textfile collector reads agent.prom                  │ │
│  │ • Exposes combined metrics at /metrics                 │ │
│  └────────────────────────────┬───────────────────────────┘ │
└────────────────────────────────┼─────────────────────────────┘
                                │
                                │ HTTP GET every 15s
                                ▼
                    ┌────────────────────┐
                    │ Prometheus         │
                    │ Scrape & Store     │
                    └────────────────────┘
```

## Log Flow Detail

```
Agent VM
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ Agent Client (logs to agentshare)                      │ │
│  │                                                          │ │
│  │ execute_command() {                                      │ │
│  │   logger.write_command(cmd_id, cmd, args);             │ │
│  │   logger.write_stdout(data);                            │ │
│  │   logger.write_stderr(data);                            │ │
│  │   logger.write_command_result(exit_code, duration);     │ │
│  │ }                                                        │ │
│  └───────────────────────────┬────────────────────────────┘ │
│                              │                              │
│                              ▼                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │ /mnt/inbox/runs/run-20260131-143022/                   │ │
│  │ ├── stdout.log   (command output)                      │ │
│  │ ├── stderr.log   (error output)                        │ │
│  │ ├── commands.log (execution log with timestamps)       │ │
│  │ └── metadata.json (run metadata)                       │ │
│  └──────────────────────┬─────────────────────────────────┘ │
└─────────────────────────┼───────────────────────────────────┘
                          │
                          │ virtiofs mount (shared storage)
                          ▼
Host System
┌──────────────────────────────────────────────────────────────┐
│  /srv/agentshare/inbox/agent-01/runs/run-20260131-143022/   │
│  ├── stdout.log                                              │
│  ├── stderr.log                                              │
│  └── commands.log                                            │
└─────────────────────┬────────────────────────────────────────┘
                      │
                      │ Tail and parse
                      ▼
     ┌────────────────────────────────┐
     │ Promtail                       │
     │ • Tail *.log files             │
     │ • Extract labels from path:    │
     │   agent_id, run_id             │
     │ • Parse JSON if present        │
     │ • Add timestamp                │
     └────────────┬───────────────────┘
                  │
                  │ HTTP Push
                  ▼
     ┌────────────────────────────────┐
     │ Loki                           │
     │ • Index by labels              │
     │ • Store log entries            │
     │ • Serve LogQL queries          │
     └────────────────────────────────┘
```

## Alert Flow Detail

```
     ┌────────────────────────────────┐
     │ Prometheus                     │
     │ • Evaluates rules every 30s    │
     │ • Checks thresholds            │
     └────────────┬───────────────────┘
                  │
                  │ Rule: AgentHighCPU > 80% for 10m
                  │ State: PENDING → FIRING
                  ▼
     ┌────────────────────────────────┐
     │ Alertmanager                   │
     │ ┌────────────────────────────┐ │
     │ │ Alert: AgentHighCPU        │ │
     │ │ Severity: warning          │ │
     │ │ Agent: agent-01            │ │
     │ │ Value: 87%                 │ │
     │ └────────────────────────────┘ │
     │                                │
     │ Group by: alertname, component │
     │ Wait: 10s for more alerts      │
     │ Route by: severity             │
     └─────────┬──────────────────────┘
               │
               │ Route decision tree
               │
    ┌──────────┴───────────┐
    │                      │
    ▼                      ▼
┌─────────┐          ┌──────────┐
│ Slack   │          │PagerDuty │
│ Channel │          │ (if      │
│         │          │ CRITICAL)│
└─────────┘          └──────────┘
    │                      │
    ▼                      ▼
"⚠ WARNING: Agent agent-01 high CPU usage (87%)"
                          │
                          ▼
              Incident created in PagerDuty
              On-call engineer paged
              Runbook link sent
```

## Component Inventory

| Component | Type | Port | Purpose | Storage |
|-----------|------|------|---------|---------|
| **Prometheus** | TSDB | 9090 | Metrics collection and querying | 100GB (90 days) |
| **Grafana** | Visualization | 3000 | Dashboards and exploration | ~1GB config |
| **Loki** | Log store | 3100 | Log aggregation and querying | ~50GB (30 days) |
| **Promtail** | Shipper | 9080 | Log collection and shipping | Stateless |
| **Alertmanager** | Alert router | 9093 | Alert routing and silencing | ~100MB state |
| **node_exporter** | Exporter | 9100 | System metrics (per VM) | Stateless |
| **Management /metrics** | Exporter | 8122 | Application metrics | Stateless |
| **Agent custom metrics** | Textfile | - | Agent-specific counters | ~1KB/agent |

## Network Ports

```
Host System (grissom)
├── Prometheus:      9090 (web UI, API)
├── Grafana:         3000 (web UI)
├── Loki:            3100 (HTTP API)
├── Promtail:        9080 (metrics)
├── Alertmanager:    9093 (web UI, API)
├── Management:      8122 (/metrics endpoint)
└── Host node_exp:   9100 (metrics)

Agent VMs (192.168.122.20X)
└── node_exporter:   9100 (metrics, scraped by Prometheus)
```

## Data Retention Summary

| Data Type | Retention | Storage Estimate | Cleanup Method |
|-----------|-----------|------------------|----------------|
| Prometheus metrics | 90 days | ~100GB | Automatic TSDB compaction |
| Loki logs | 30 days | ~50GB | Retention policy |
| Agent run logs (inbox) | Until task deleted | ~10GB/week | Task cleanup on completion |
| Grafana dashboards | Permanent | ~100MB | Manual deletion |
| Alertmanager state | 24 hours | ~100MB | Automatic |

---

**Legend:**
- `→` = Data flow
- `┌─┐` = Component boundary
- `├─┤` = Hierarchical structure
- `▼` = Directional flow

# Observability System Documentation

**Comprehensive monitoring, logging, and alerting design for agentic-sandbox**

**Project:** Agentic Sandbox VM Orchestration Platform
**Author:** Reliability Engineer
**Date:** 2026-01-31
**Status:** Design Complete - Ready for Implementation

---

## Overview

This directory contains the complete observability system design for the agentic-sandbox platform, including metrics collection, log aggregation, SLI/SLO definitions, alert rules, and operational procedures.

**Key Features:**
- Host-based metrics aggregation using Prometheus
- Per-agent custom metrics via node_exporter textfile collector
- Centralized log shipping to Loki
- Actionable SLIs/SLOs for agent availability and task success
- Three-tier alerting (warning, critical, emergency)
- Production-ready dashboards and runbooks

---

## Deliverables

### 📋 Design Documents (3)

1. **[OBSERVABILITY_DESIGN.md](../OBSERVABILITY_DESIGN.md)** *(1,568 lines)*
   - **Purpose:** Complete observability architecture and design specification
   - **Contents:**
     - Current state analysis
     - Architecture overview with diagrams
     - Metrics collection design (management server + agent VMs)
     - Log aggregation strategy (Loki + Promtail)
     - SLI/SLO definitions with error budget policy
     - Alert rules with severity levels
     - Dashboard specifications
     - 8-week implementation roadmap
   - **Audience:** Engineering leads, DevOps team, reliability engineers

2. **[ARCHITECTURE_DIAGRAM.md](ARCHITECTURE_DIAGRAM.md)** *(28 KB)*
   - **Purpose:** Visual architecture reference with ASCII diagrams
   - **Contents:**
     - High-level system overview
     - Data flow diagrams (metrics, logs, alerts)
     - Component inventory with ports and storage
     - Network topology
     - Retention policies
   - **Audience:** All engineers, operators, stakeholders

3. **[QUICK_REFERENCE.md](QUICK_REFERENCE.md)** *(9.5 KB)*
   - **Purpose:** Operator cheat sheet for daily operations
   - **Contents:**
     - Key PromQL and LogQL queries
     - Common operational tasks
     - Troubleshooting procedures
     - Performance baselines
     - Contact information
   - **Audience:** On-call engineers, operators

### 🔧 Configuration Files (3)

4. **[prometheus.yml](prometheus.yml)** *(3.5 KB)*
   - **Purpose:** Prometheus scrape configuration
   - **Deploy to:** `/etc/prometheus/prometheus.yml`
   - **Features:**
     - Management server scrape config
     - Agent VM discovery (static + file_sd)
     - Alert rule loading
     - 90-day retention
     - Optional remote write config

5. **[alert-rules.yml](alert-rules.yml)** *(15 KB)*
   - **Purpose:** Prometheus alerting rules
   - **Deploy to:** `/etc/prometheus/rules/agentic-sandbox.yml`
   - **Features:**
     - 25+ alert rules across 7 categories
     - Agent health (CPU, memory, disk, connectivity)
     - Command execution (failure rate, latency, stalls)
     - Task orchestration (backlog, failures)
     - Management server health
     - SLO violations
     - Storage quotas
   - **Alert count:** 25 rules
   - **Severity levels:** WARNING (14), CRITICAL (10), EMERGENCY (1)

6. **[file_sd_targets_example.json](file_sd_targets_example.json)** *(241 bytes)*
   - **Purpose:** Example Prometheus file-based service discovery
   - **Deploy to:** `/etc/prometheus/targets/agents.json`
   - **Use case:** Dynamic agent registration

### 📝 Implementation Guides (1)

7. **[IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)** *(15 KB)*
   - **Purpose:** Step-by-step implementation guide with checklists
   - **Timeline:** 8 weeks (6 phases + post-implementation)
   - **Contents:**
     - Phase 1: Foundation (Week 1-2) - Prometheus, Grafana, node_exporter
     - Phase 2: Custom Metrics (Week 3) - Agent-side exporters, management extensions
     - Phase 3: Log Aggregation (Week 4) - Loki, Promtail, JSON logging
     - Phase 4: SLI/SLO Implementation (Week 5) - Recording rules, dashboards
     - Phase 5: Alerting (Week 6) - Alertmanager, runbooks, testing
     - Phase 6: Production Hardening (Week 7-8) - Retention, backup, ORR
   - **Checkboxes:** 87 actionable items
   - **Sign-off gates:** 7 approval points

---

## Quick Start

### For Engineering Leads

1. **Review Design:** Read [OBSERVABILITY_DESIGN.md](../OBSERVABILITY_DESIGN.md)
2. **Approve SLOs:** Section 5 (SLI/SLO Definitions)
3. **Assign Owner:** Designate reliability engineer to lead implementation
4. **Schedule Kickoff:** Plan 8-week timeline starting from approved date

### For Implementation Team

1. **Read Checklist:** [IMPLEMENTATION_CHECKLIST.md](IMPLEMENTATION_CHECKLIST.md)
2. **Set Up Environment:**
   ```bash
   # Install Prometheus
   sudo apt install prometheus grafana -y

   # Deploy configs
   sudo cp prometheus.yml /etc/prometheus/prometheus.yml
   sudo cp alert-rules.yml /etc/prometheus/rules/agentic-sandbox.yml
   sudo systemctl reload prometheus
   ```
3. **Follow Phase 1:** Start with Foundation (Week 1-2)
4. **Track Progress:** Check off items in IMPLEMENTATION_CHECKLIST.md

### For Operators

1. **Bookmark:** [QUICK_REFERENCE.md](QUICK_REFERENCE.md)
2. **Access Dashboards:**
   - Prometheus: http://localhost:9090
   - Grafana: http://localhost:3000
   - Alertmanager: http://localhost:9093
3. **Join Channels:**
   - `#agentic-sandbox-alerts` (Slack)
   - `#agentic-sandbox-incidents` (Slack)

---

## Architecture Summary

```
┌─────────────────────────────────────────────────────────────┐
│                     Observability Stack                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐             │
│  │Prometheus│───▶│Alertmgr  │───▶│  Slack   │             │
│  │ (Metrics)│    │ (Alerts) │    │PagerDuty │             │
│  └────┬─────┘    └──────────┘    └──────────┘             │
│       │                                                     │
│       │ Scrape                                              │
│       │                                                     │
│  ┌────▼─────┐    ┌──────────┐    ┌──────────┐             │
│  │  Mgmt    │    │  Agents  │    │   Host   │             │
│  │ Server   │    │node_exp  │    │node_exp  │             │
│  │/metrics  │    │ :9100    │    │ :9100    │             │
│  └──────────┘    └──────────┘    └──────────┘             │
│                                                             │
│  ┌──────────┐    ┌──────────┐                              │
│  │   Loki   │◀───│ Promtail │                              │
│  │  (Logs)  │    │ (Shipper)│                              │
│  └────┬─────┘    └──────────┘                              │
│       │                                                     │
│       │ Query                                               │
│       │                                                     │
│  ┌────▼───────────────────────────────────┐                │
│  │           Grafana Dashboards           │                │
│  │  • Agent Fleet Overview                │                │
│  │  • Task Orchestration                  │                │
│  │  • Storage & Quotas                    │                │
│  │  • SLO Compliance                      │                │
│  └────────────────────────────────────────┘                │
└─────────────────────────────────────────────────────────────┘
```

---

## Key Metrics

### Management Server Metrics (Existing)

| Metric | Type | Description |
|--------|------|-------------|
| `agentic_uptime_seconds` | gauge | Server uptime |
| `agentic_agents_connected` | gauge | Connected agent count |
| `agentic_agents_by_status{status}` | gauge | Agents by status (ready, busy) |
| `agentic_commands_total` | counter | Total commands dispatched |
| `agentic_commands_by_result{result}` | counter | Commands by result (success, failed) |
| `agentic_tasks_by_state{state}` | gauge | Tasks by state |

### Custom Agent Metrics (New)

| Metric | Type | Description |
|--------|------|-------------|
| `agentic_agent_commands_total{agent_id}` | counter | Commands executed per agent |
| `agentic_agent_commands_success{agent_id}` | counter | Successful commands per agent |
| `agentic_agent_claude_tasks_total{agent_id}` | counter | Claude tasks per agent |
| `agentic_agent_current_commands{agent_id}` | gauge | Active commands per agent |

### System Metrics (node_exporter)

| Metric | Type | Description |
|--------|------|-------------|
| `node_cpu_seconds_total{mode}` | counter | CPU time by mode |
| `node_memory_MemAvailable_bytes` | gauge | Available memory |
| `node_filesystem_avail_bytes{mountpoint}` | gauge | Available disk space |
| `node_network_transmit_bytes_total{device}` | counter | Network TX bytes |

---

## SLO Targets

| SLO | Target | Error Budget | Measurement Window |
|-----|--------|--------------|-------------------|
| **Agent Availability** | 99.0% | 100.8 min/week | Rolling 7 days |
| **Command Success Rate** | 99.0% | 14.4 min/day | Rolling 24 hours |
| **Task Success Rate** | 95.0% | 8.4 hours/week | Rolling 7 days |
| **Management Server Uptime** | 99.9% | 43 min/month | Rolling 30 days |

**Error Budget Policy:**
- **> 50% remaining:** Normal development velocity
- **25-50% remaining:** Freeze risky deployments
- **10-25% remaining:** CRITICAL - All non-essential changes blocked
- **< 10% remaining:** EMERGENCY - Rollback + incident commander

---

## Alert Summary

### By Severity

| Severity | Count | Notification | Response Time |
|----------|-------|--------------|---------------|
| **WARNING** | 14 | Slack | Best-effort |
| **CRITICAL** | 10 | PagerDuty | < 30 minutes |
| **EMERGENCY** | 1 | PagerDuty + Slack + SMS | Immediate |

### By Category

| Category | Alert Count |
|----------|-------------|
| Agent Health | 6 |
| Command Execution | 4 |
| Task Orchestration | 3 |
| Management Server | 4 |
| SLO Violations | 3 |
| Storage Quotas | 3 |
| Network | 2 |

---

## Implementation Timeline

```
Week 1-2:  Foundation         [Prometheus, Grafana, node_exporter]
Week 3:    Custom Metrics     [Agent exporters, management extensions]
Week 4:    Log Aggregation    [Loki, Promtail, JSON logs]
Week 5:    SLI/SLO            [Recording rules, dashboards]
Week 6:    Alerting           [Alertmanager, runbooks, testing]
Week 7-8:  Hardening          [Retention, backup, ORR]
───────────────────────────────────────────────────────────────
Total:     8 weeks            [87 checklist items, 7 sign-offs]
```

---

## Dependencies

### Software Requirements

| Component | Minimum Version | Install Method |
|-----------|----------------|----------------|
| Prometheus | 2.50.0 | `apt install prometheus` |
| Grafana | 10.3.0 | `apt install grafana` |
| Loki | 2.9.0 | Docker or binary |
| Promtail | 2.9.0 | `apt install promtail` |
| Alertmanager | 0.27.0 | `apt install prometheus-alertmanager` |
| node_exporter | 1.7.0 | `apt install prometheus-node-exporter` |

### Infrastructure Requirements

| Resource | Requirement | Notes |
|----------|-------------|-------|
| Disk (Prometheus) | 100GB | 90-day retention |
| Disk (Loki) | 50GB | 30-day retention |
| RAM (Prometheus) | 4GB | With 50 agents |
| RAM (Loki) | 2GB | With 10GB/day ingestion |
| Network | 1 Gbps | Between host and agents |

---

## Success Metrics

### Technical Metrics

- [ ] **100%** of agent VMs monitored
- [ ] **25+** alert rules configured
- [ ] **4** production dashboards created
- [ ] **< 10 seconds** alert fire-to-notification latency
- [ ] **< 60 seconds** log write-to-query latency

### Process Metrics

- [ ] **7** ORR checklist items approved
- [ ] **10+** runbooks written
- [ ] **100%** of critical alerts tested
- [ ] **< 1 hour** recovery time objective from backup
- [ ] **12-month** capacity plan approved

---

## Related Documentation

| Document | Location | Purpose |
|----------|----------|---------|
| Provisioning Scripts | `/images/qemu/provision-vm.sh` | VM setup |
| Management Server | `/management/README.md` | Server architecture |
| Agent Client | `/agent-rs/README.md` | Agent implementation |
| Protocol Spec | `/proto/agent.proto` | gRPC messages |
| Task Lifecycle | `/docs/TASK_LIFECYCLE.md` | Orchestration design |

---

## Support & Feedback

**Questions?** Contact the platform team:
- Engineering Lead: [Name/Email]
- DevOps Lead: [Name/Email]
- On-Call: PagerDuty rotation

**Found an issue?** Open a ticket:
- Gitea: https://git.integrolabs.net/roctinam/agentic-sandbox/issues

**Contributing:** Follow conventional commit format:
```
feat(observability): add custom agent metrics exporter
docs(observability): update SLO targets
fix(alerts): correct AgentHighCPU threshold
```

---

## Changelog

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-01-31 | Initial design complete |
| - | - | Awaiting implementation kickoff |

---

**Status:** ✅ Design Complete - Ready for Implementation

**Next Steps:**
1. Schedule design review meeting
2. Assign implementation owner
3. Approve 8-week timeline and budget
4. Kick off Phase 1 (Foundation)

---

**Last Updated:** 2026-01-31
**Document Owner:** Reliability Engineer

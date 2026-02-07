# Observability Implementation Checklist

**Project:** Agentic Sandbox Observability System
**Owner:** Reliability Engineer
**Start Date:** 2026-01-31
**Target Completion:** 2026-03-31 (8 weeks)

---

## Phase 1: Foundation (Week 1-2)

### Host Infrastructure Setup

- [ ] **1.1** Install Prometheus on host system
  ```bash
  sudo apt update
  sudo apt install prometheus prometheus-alertmanager -y
  ```
  - [ ] Verify installation: `prometheus --version`
  - [ ] Verify service running: `systemctl status prometheus`

- [ ] **1.2** Configure Prometheus
  - [ ] Deploy `/home/roctinam/dev/agentic-sandbox/docs/observability/prometheus.yml` to `/etc/prometheus/prometheus.yml`
  - [ ] Create targets directory: `sudo mkdir -p /etc/prometheus/targets`
  - [ ] Create rules directory: `sudo mkdir -p /etc/prometheus/rules`
  - [ ] Reload Prometheus: `sudo systemctl reload prometheus`
  - [ ] Verify config: `promtool check config /etc/prometheus/prometheus.yml`

- [ ] **1.3** Install Grafana
  ```bash
  sudo apt install grafana -y
  sudo systemctl enable grafana-server
  sudo systemctl start grafana-server
  ```
  - [ ] Access Grafana UI: http://localhost:3000 (admin/admin)
  - [ ] Change default password
  - [ ] Add Prometheus data source (http://localhost:9090)

- [ ] **1.4** Verify management server metrics
  - [ ] Start management server: `cd management && ./dev.sh`
  - [ ] Check metrics endpoint: `curl http://localhost:8122/metrics`
  - [ ] Verify metrics appear in Prometheus: http://localhost:9090/targets

### Agent VM Setup

- [ ] **1.5** Update VM provisioning script
  - [ ] Edit `images/qemu/profiles/agentic-dev/packages.txt`
  - [ ] Add line: `prometheus-node-exporter`
  - [ ] Commit change

- [ ] **1.6** Provision test agent VM
  ```bash
  ./images/qemu/provision-vm.sh agent-test-01 --profile agentic-dev --agentshare --start
  ```
  - [ ] Wait for provisioning to complete (~10 minutes)
  - [ ] SSH into VM: `ssh agent@192.168.122.201`
  - [ ] Verify node_exporter running: `systemctl status prometheus-node-exporter`
  - [ ] Test metrics endpoint: `curl http://localhost:9100/metrics`

- [ ] **1.7** Add agent to Prometheus targets
  - [ ] Edit `/etc/prometheus/prometheus.yml` (uncomment agent-vms section)
  - [ ] Or create `/etc/prometheus/targets/agents.json`:
    ```json
    [{"targets": ["192.168.122.201:9100"], "labels": {"agent_id": "agent-test-01"}}]
    ```
  - [ ] Reload Prometheus: `sudo systemctl reload prometheus`
  - [ ] Verify target UP: http://localhost:9090/targets

### Initial Dashboards

- [ ] **1.8** Import pre-built dashboards to Grafana
  - [ ] Node Exporter Full (Dashboard ID: 1860)
  - [ ] Prometheus Stats (Dashboard ID: 2)
  - [ ] Verify agent metrics visible

- [ ] **1.9** Test end-to-end metrics flow
  - [ ] Run test command on agent: `ssh agent@192.168.122.201 'uptime'`
  - [ ] Query agent CPU in Prometheus:
    ```promql
    rate(node_cpu_seconds_total{agent_id="agent-test-01"}[5m])
    ```
  - [ ] View in Grafana dashboard

**Phase 1 Sign-off:** [ ] Engineering Lead

---

## Phase 2: Custom Metrics (Week 3)

### Agent-Side Custom Metrics

- [ ] **2.1** Implement custom metrics exporter
  - [ ] Create `agent-rs/src/metrics_exporter.rs` module
  - [ ] Implement `AgentMetricsExporter` struct
  - [ ] Add textfile write logic (Prometheus format)
  - [ ] Wire into `agent-rs/src/main.rs`

- [ ] **2.2** Configure node_exporter textfile collector
  - [ ] SSH to agent VM: `ssh agent@192.168.122.201`
  - [ ] Create directory: `sudo mkdir -p /var/lib/prometheus/node-exporter`
  - [ ] Set permissions: `sudo chown agent:agent /var/lib/prometheus/node-exporter`
  - [ ] Enable textfile collector in `/etc/default/prometheus-node-exporter`:
    ```bash
    ARGS="--collector.textfile.directory=/var/lib/prometheus/node-exporter"
    ```
  - [ ] Restart node_exporter: `sudo systemctl restart prometheus-node-exporter`

- [ ] **2.3** Deploy and test custom exporter
  - [ ] Build agent client: `cd agent-rs && cargo build --release`
  - [ ] Deploy to VM: `scp target/release/agent-client agent@192.168.122.201:/tmp/`
  - [ ] SSH and restart agent: `sudo systemctl restart agentic-agent`
  - [ ] Wait 60 seconds (first metrics write)
  - [ ] Check textfile: `cat /var/lib/prometheus/node-exporter/agent.prom`
  - [ ] Verify metrics in Prometheus:
    ```promql
    agentic_agent_commands_total{agent_id="agent-test-01"}
    ```

### Management Server Metrics Extensions

- [ ] **2.4** Add session tracking metrics
  - [ ] Edit `management/src/telemetry/metrics.rs`
  - [ ] Add fields: `agent_session_count`, `agent_session_duration_sum_ms`, `agent_restarts_total`
  - [ ] Implement methods: `agent_session_started()`, `agent_session_ended()`, `agent_restarted()`
  - [ ] Wire into `management/src/registry.rs` on connect/disconnect events

- [ ] **2.5** Add storage metrics
  - [ ] Add field: `agentshare_inbox_bytes: HashMap<String, AtomicU64>`
  - [ ] Update on agent heartbeat (parse from `Metrics` message)
  - [ ] Export in `prometheus_format()` method

- [ ] **2.6** Add command latency histogram
  - [ ] Add field: `command_latency_buckets: [AtomicU64; 10]`
  - [ ] Buckets: 0.01, 0.05, 0.1, 0.5, 1, 5, 10, 30, 60, +Inf seconds
  - [ ] Record latency in `command_completed()` method
  - [ ] Export histogram in Prometheus format

- [ ] **2.7** Build and deploy management server
  ```bash
  cd management
  ./dev.sh restart
  ```
  - [ ] Verify new metrics: `curl http://localhost:8122/metrics | grep session`
  - [ ] Check Prometheus scrape

**Phase 2 Sign-off:** [ ] Engineering Lead

---

## Phase 3: Log Aggregation (Week 4)

### Loki Deployment

- [ ] **3.1** Deploy Loki on host
  ```bash
  # Option 1: Docker (simple)
  docker run -d --name=loki -p 3100:3100 grafana/loki:latest

  # Option 2: Native install (production)
  wget https://github.com/grafana/loki/releases/download/v2.9.0/loki-linux-amd64.zip
  unzip loki-linux-amd64.zip
  sudo mv loki-linux-amd64 /usr/local/bin/loki
  sudo systemctl enable loki
  sudo systemctl start loki
  ```
  - [ ] Verify Loki: `curl http://localhost:3100/ready`

- [ ] **3.2** Install Promtail
  ```bash
  sudo apt install promtail -y
  ```
  - [ ] Deploy config to `/etc/promtail/config.yml`
  - [ ] Reload: `sudo systemctl restart promtail`
  - [ ] Check status: `systemctl status promtail`

- [ ] **3.3** Add Loki data source to Grafana
  - [ ] Open Grafana → Configuration → Data Sources
  - [ ] Add Loki: http://localhost:3100
  - [ ] Test connection

### Agent Logging Enhancements

- [ ] **3.4** Enable JSON logging in agent client
  - [ ] Edit `agent-rs/src/main.rs` (lines 1545-1584)
  - [ ] Change default log format to JSON
  - [ ] Add structured fields: `agent_id`, `command_id`, etc.
  - [ ] Build and deploy

- [ ] **3.5** Configure agent log shipping
  - [ ] Edit `/etc/promtail/config.yml` on host
  - [ ] Add scrape config for `/srv/agentshare/inbox/*/runs/*/stdout.log`
  - [ ] Add scrape config for `/srv/agentshare/inbox/*/runs/*/stderr.log`
  - [ ] Add scrape config for `/srv/agentshare/inbox/*/runs/*/commands.log`
  - [ ] Reload Promtail

- [ ] **3.6** Test log ingestion
  - [ ] Run test command on agent
  - [ ] Wait 30 seconds for Promtail scrape
  - [ ] Query logs in Grafana Explore:
    ```logql
    {agent_id="agent-test-01"} |= "Command completed"
    ```
  - [ ] Verify log volume in Loki metrics

### Log Retention & Rotation

- [ ] **3.7** Configure Loki retention
  - [ ] Edit Loki config: set `retention_period: 720h` (30 days)
  - [ ] Restart Loki

- [ ] **3.8** Configure agent logrotate
  - [ ] Create `/etc/logrotate.d/agentic-agent` on agent VM
  - [ ] Set rotation: daily, keep 7 days
  - [ ] Test: `logrotate -d /etc/logrotate.d/agentic-agent`

**Phase 3 Sign-off:** [ ] DevOps Lead

---

## Phase 4: SLI/SLO Implementation (Week 5)

### Define Recording Rules

- [ ] **4.1** Create SLI recording rules
  - [ ] Create `/etc/prometheus/rules/sli.yml`
  - [ ] Add rules for:
    - [ ] `sli:agent_availability:5m`
    - [ ] `sli:command_success_rate:5m`
    - [ ] `sli:task_success_rate:1h`
  - [ ] Reload Prometheus: `sudo systemctl reload prometheus`
  - [ ] Verify rules: http://localhost:9090/rules

- [ ] **4.2** Create error budget calculation rule
  - [ ] Add rule: `slo:error_budget_remaining:percentage`
  - [ ] Formula: `1 - ((failures / total) / (1 - SLO_TARGET))`
  - [ ] Test query in Prometheus

### Build SLO Dashboard

- [ ] **4.3** Create SLO Compliance dashboard in Grafana
  - [ ] Panel 1: Agent Availability SLI (99% target line)
  - [ ] Panel 2: Command Success Rate SLI (99% target line)
  - [ ] Panel 3: Task Success Rate SLI (95% target line)
  - [ ] Panel 4: Error Budget Remaining (gauge, thresholds at 50%, 25%, 10%)
  - [ ] Panel 5: SLO Violation Log (table from alert history)

- [ ] **4.4** Document SLO targets
  - [ ] Create runbook: `/docs/runbooks/SLO_COMPLIANCE.md`
  - [ ] Document error budget policy
  - [ ] Define escalation procedures

### Test SLO Tracking

- [ ] **4.5** Generate test load
  - [ ] Run 100 successful commands
  - [ ] Run 5 failing commands (simulate 5% failure rate)
  - [ ] Verify SLI drops below 99%
  - [ ] Check error budget calculation

**Phase 4 Sign-off:** [ ] Product Owner

---

## Phase 5: Alerting (Week 6)

### Alertmanager Configuration

- [ ] **5.1** Configure Alertmanager
  - [ ] Edit `/etc/alertmanager/alertmanager.yml`
  - [ ] Add Slack webhook URL
  - [ ] Add PagerDuty service key (if available)
  - [ ] Configure routing tree (WARNING → Slack, CRITICAL → PagerDuty)
  - [ ] Reload: `sudo systemctl reload alertmanager`

- [ ] **5.2** Deploy alert rules
  - [ ] Copy `/home/roctinam/dev/agentic-sandbox/docs/observability/alert-rules.yml`
  - [ ] To: `/etc/prometheus/rules/agentic-sandbox.yml`
  - [ ] Reload Prometheus: `sudo systemctl reload prometheus`
  - [ ] Verify rules: http://localhost:9090/alerts

### Test Alerting

- [ ] **5.3** Test WARNING alert (AgentHighCPU)
  - [ ] SSH to agent: `ssh agent@192.168.122.201`
  - [ ] Generate CPU load: `stress-ng --cpu 4 --timeout 15m`
  - [ ] Wait for alert to fire (10 minutes)
  - [ ] Verify Slack notification received
  - [ ] Verify alert appears in Grafana

- [ ] **5.4** Test CRITICAL alert (AgentDown)
  - [ ] Stop node_exporter: `sudo systemctl stop prometheus-node-exporter`
  - [ ] Wait for alert to fire (2 minutes)
  - [ ] Verify PagerDuty incident created (if configured)
  - [ ] Verify Slack notification
  - [ ] Restart exporter: `sudo systemctl start prometheus-node-exporter`
  - [ ] Verify alert resolves

- [ ] **5.5** Test alert inhibition
  - [ ] Trigger multiple related alerts
  - [ ] Verify only highest severity alert fires
  - [ ] Check Alertmanager silences

### Create Runbooks

- [ ] **5.6** Write runbooks for each alert
  - [ ] AgentHighCPU runbook
  - [ ] AgentDown runbook
  - [ ] HighCommandFailureRate runbook
  - [ ] TaskQueueBacklog runbook
  - [ ] ManagementServerDown runbook
  - [ ] ErrorBudgetDepleted runbook
  - [ ] Store in `/docs/runbooks/`

**Phase 5 Sign-off:** [ ] On-Call Team Lead

---

## Phase 6: Production Hardening (Week 7-8)

### Storage & Retention Tuning

- [ ] **6.1** Configure Prometheus retention
  - [ ] Edit `/etc/default/prometheus`:
    ```bash
    ARGS="--storage.tsdb.retention.time=90d --storage.tsdb.retention.size=100GB"
    ```
  - [ ] Restart Prometheus
  - [ ] Verify retention: http://localhost:9090/config

- [ ] **6.2** Set up Prometheus backup
  - [ ] Create backup script: `/usr/local/bin/prometheus-backup.sh`
  - [ ] Script: `tar -czf /backup/prometheus-$(date +%Y%m%d).tar.gz /var/lib/prometheus/data`
  - [ ] Add cron job: `0 2 * * * /usr/local/bin/prometheus-backup.sh`
  - [ ] Test backup: `sudo /usr/local/bin/prometheus-backup.sh`

- [ ] **6.3** Test disaster recovery
  - [ ] Stop Prometheus
  - [ ] Restore from backup
  - [ ] Verify data integrity
  - [ ] Document RTO (Recovery Time Objective)

### Capacity Planning

- [ ] **6.4** Establish performance baselines
  - [ ] Run 7-day load test
  - [ ] Record metrics:
    - [ ] Prometheus storage growth rate (GB/day)
    - [ ] Query latency P95
    - [ ] Agent resource usage under load
    - [ ] Management server resource usage
  - [ ] Document in `/docs/CAPACITY_PLAN.md`

- [ ] **6.5** Set resource quotas
  - [ ] Define CPU limits per agent (e.g., 4 cores)
  - [ ] Define memory limits per agent (e.g., 8GB)
  - [ ] Define disk limits per agent (e.g., 40GB)
  - [ ] Configure in provisioning script

### Operational Readiness Review (ORR)

- [ ] **6.6** Pre-ORR tasks
  - [ ] Complete all checklist items above
  - [ ] Run end-to-end smoke test
  - [ ] Verify all dashboards functional
  - [ ] Verify all alerts tested
  - [ ] Document on-call procedures

- [ ] **6.7** ORR checklist
  - [ ] Monitoring covers all SLIs: **[ ] YES / [ ] NO**
  - [ ] Alerts tested and functional: **[ ] YES / [ ] NO**
  - [ ] Runbooks written for all critical alerts: **[ ] YES / [ ] NO**
  - [ ] On-call rotation established: **[ ] YES / [ ] NO**
  - [ ] Escalation procedures documented: **[ ] YES / [ ] NO**
  - [ ] Disaster recovery tested: **[ ] YES / [ ] NO**
  - [ ] Capacity plan approved: **[ ] YES / [ ] NO**

- [ ] **6.8** ORR sign-off
  - [ ] Engineering Lead
  - [ ] DevOps Lead
  - [ ] Security Team
  - [ ] Operations Lead
  - [ ] VP Engineering

**Phase 6 Sign-off:** [ ] VP Engineering

---

## Post-Implementation

### Documentation

- [ ] **7.1** Update main README.md with observability section
- [ ] **7.2** Create operator guide: `/docs/OPERATOR_GUIDE.md`
- [ ] **7.3** Record demo video of dashboard usage
- [ ] **7.4** Schedule knowledge transfer session with team

### Continuous Improvement

- [ ] **7.5** Schedule monthly SLO review meeting
- [ ] **7.6** Set up quarterly capacity planning review
- [ ] **7.7** Establish process for adding new metrics
- [ ] **7.8** Plan for future enhancements:
  - [ ] Distributed tracing (Jaeger/Tempo)
  - [ ] Cost tracking per agent
  - [ ] Anomaly detection with ML

---

## Success Criteria

| Criterion | Target | Status |
|-----------|--------|--------|
| All agent VMs monitored | 100% | [ ] |
| Management server metrics exposed | /metrics endpoint | [ ] |
| Custom metrics exported | 10+ custom metrics | [ ] |
| Logs centralized | All runs logged to Loki | [ ] |
| SLOs defined | 3 critical SLOs | [ ] |
| Alerts configured | 20+ alert rules | [ ] |
| Dashboards created | 4 production dashboards | [ ] |
| Runbooks written | 1 per critical alert | [ ] |
| ORR passed | All reviewers signed off | [ ] |

---

## Notes

**Blockers:**
- None

**Risks:**
- Prometheus storage growth may exceed 100GB if retention too high
- Loki ingestion may lag if too many agents writing logs simultaneously

**Mitigation:**
- Monitor Prometheus storage usage weekly
- Implement log sampling if volume exceeds 10GB/day

**Contacts:**
- Engineering Lead: [Name]
- DevOps Lead: [Name]
- On-Call Rotation: [PagerDuty link]

---

**Last Updated:** 2026-01-31

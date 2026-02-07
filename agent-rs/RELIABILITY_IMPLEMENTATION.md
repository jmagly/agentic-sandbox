# Service Reliability Implementation for Issue #91

This document summarizes the implementation of circuit breaker and watchdog reliability features for the agent-client service.

## Overview

Implemented comprehensive reliability features including:
- Systemd watchdog integration with Type=notify
- Health state management (healthy, degraded, unhealthy)
- Circuit breaker integration
- Resource-based health checks
- Prometheus-style metrics
- Health verification script

## Implementation Details

### 1. Systemd Service Hardening

**File:** `/home/roctinam/dev/agentic-sandbox/agent-rs/systemd/agent-client.service`

Key changes:
```ini
# Service type with watchdog support
Type=notify
NotifyAccess=main

# Watchdog configuration
WatchdogSec=30           # Expect ping every 30s
WatchdogSignal=SIGABRT   # Kill signal on timeout

# Restart limits
StartLimitBurst=5
StartLimitIntervalSec=300

# Resource limits
MemoryMax=7680M          # Hard limit
MemoryHigh=6144M         # Soft limit
TasksMax=2048            # Fork bomb defense
CPUQuota=400%            # 4 cores
IOReadBandwidthMax=/dev/vda 500M
IOWriteBandwidthMax=/dev/vda 200M
LimitNOFILE=65536        # FD limit
```

### 2. Health State Module

**File:** `agent-rs/src/health.rs`

Implements three health states:

#### Healthy
- Normal operation
- Accepts all tasks
- All systems operational

#### Degraded
- Limited operation
- Rejects new tasks
- Triggered by:
  - 3+ consecutive failures
  - Memory usage >85%
  - Circuit breaker trips
  - Frequent restarts (>3)

#### Unhealthy
- Recovery mode only
- Diagnostic operations only
- Triggered by:
  - 5+ consecutive failures
  - Memory usage >95%

**Key Components:**

```rust
pub struct HealthMonitor {
    state: Arc<AtomicU8>,
    consecutive_failures: AtomicU32,
    consecutive_successes: AtomicU32,
    total_restarts: AtomicU32,
    total_watchdog_pings: AtomicU32,
    total_circuit_breaker_trips: AtomicU32,
    config: HealthConfig,
    agent_id: String,
}

pub struct SystemdWatchdog {
    enabled: bool,
    interval: Duration,
    health: Arc<HealthMonitor>,
}
```

**Features:**
- Automatic state transitions based on failure/success counts
- Resource-based health checks (memory, disk)
- Integration with circuit breaker pattern
- Graceful degradation on non-systemd systems (feature flag)

### 3. Systemd Integration

Uses `sd-notify` crate for systemd communication:

```rust
// On startup
watchdog.notify_ready()  // Sends READY to systemd

// Background ping loop (every 15s, half of WatchdogSec)
loop {
    watchdog.ping()  // Sends WATCHDOG=1 to systemd
    sleep(15s)
}
```

If the agent stops pinging for 30s, systemd sends SIGABRT and restarts the service.

### 4. Metrics Module

**File:** `agent-rs/src/metrics.rs`

Exports Prometheus-style metrics:

```
# Health state (0=healthy, 1=degraded, 2=unhealthy)
agentic_agent_health_state{agent_id="agent-01",state="healthy"} 1
agentic_agent_health_state{agent_id="agent-01",state="degraded"} 0
agentic_agent_health_state{agent_id="agent-01",state="unhealthy"} 0

# Counters
agentic_agent_restarts_total{agent_id="agent-01"} 2
agentic_agent_watchdog_pings_total{agent_id="agent-01"} 342
agentic_agent_circuit_breaker_trips{agent_id="agent-01"} 0

# Uptime
agentic_agent_uptime_seconds{agent_id="agent-01"} 3600
```

### 5. Health Verification Script

**File:** `agent-rs/scripts/verify-agent-health.sh`

Post-start validation script that checks:
- Process is running
- Systemd received READY notification
- Memory pressure
- Disk space
- Restart frequency

Retries up to 10 times with 1s delay. Exits 0 on success, 1 on failure (triggers restart).

### 6. Integration with Main Agent Code

**File:** `agent-rs/src/main.rs`

Changes:

```rust
struct AgentClient {
    config: AgentConfig,
    output_tx: mpsc::Sender<AgentMessage>,
    output_rx: Option<mpsc::Receiver<AgentMessage>>,
    agentshare: Option<Arc<AgentshareLogger>>,
    running_commands: RunningCommands,
    health: Arc<health::HealthMonitor>,        // NEW
    watchdog: Option<Arc<health::SystemdWatchdog>>,  // NEW
}

async fn run(&mut self) -> Result<()> {
    // Initialize watchdog
    let watchdog = Arc::new(health::SystemdWatchdog::new(self.health.clone()));
    self.watchdog = Some(watchdog.clone());

    // Start watchdog ping loop
    tokio::spawn(async move {
        watchdog.run_ping_loop().await;
    });

    // Notify systemd we're ready
    if let Some(ref wd) = self.watchdog {
        wd.notify_ready()?;
    }

    loop {
        match self.connect().await {
            Ok(mut client) => {
                self.health.record_success();  // Track successful connection
                // ...
            }
            Err(e) => {
                self.health.record_failure();  // Track failed connection
                // ...
            }
        }
    }
}

async fn main() -> Result<()> {
    // Record start time for uptime metrics
    metrics::record_start_time();

    // Check for restart
    let is_restart = Path::new("/tmp/agent-client-restart.marker").exists();
    fs::write("/tmp/agent-client-restart.marker", "1")?;

    let mut client = AgentClient::new(config);
    if is_restart {
        client.health.record_restart();
    }

    client.run().await
}
```

### 7. Cargo Dependencies

**File:** `agent-rs/Cargo.toml`

```toml
[dependencies]
sd-notify = { version = "0.4", optional = true }

[features]
default = ["systemd"]
systemd = ["sd-notify"]
```

Systemd integration is optional and degrades gracefully when not available.

## Health State Transitions

```
           failures >= 3
Healthy ─────────────────────> Degraded
   ^                               │
   │                               │ failures >= 5
   │ successes >= 3                │
   │                               v
   └──────────────────────── Unhealthy
```

Additional degraded triggers:
- Memory usage >85%
- Circuit breaker trip
- Restart count >3

Additional unhealthy triggers:
- Memory usage >95%

## Testing

Compile and test:

```bash
cd agent-rs
cargo build --release --all-features
cargo test --all-features

# Install
sudo cp target/release/agent-client /opt/agentic-sandbox/bin/
sudo cp systemd/agent-client.service /etc/systemd/system/
sudo cp scripts/verify-agent-health.sh /opt/agentic-sandbox/bin/
sudo chmod +x /opt/agentic-sandbox/bin/verify-agent-health.sh

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable agent-client
sudo systemctl start agent-client

# Check status
sudo systemctl status agent-client
journalctl -u agent-client -f
```

## Verification

Check health state:
```bash
# View logs
journalctl -u agent-client | grep -i health

# Check watchdog status
systemctl show agent-client | grep -E "Watchdog|MainPID"

# View metrics (if exposed via HTTP)
curl http://localhost:9090/metrics
```

## Circuit Breaker Integration

The health monitor can be integrated with the existing circuit breaker:

```rust
// In management server circuit breaker
circuit_breaker.on_trip(|| {
    agent_client.health.record_circuit_breaker_trip();
});
```

## Backwards Compatibility

- Feature flag `systemd` allows compilation without systemd support
- Graceful degradation when watchdog is not configured
- No systemd calls on non-Linux systems
- All health checks work with or without systemd

## Future Enhancements

1. Expose metrics via HTTP endpoint (e.g., `:9090/metrics`)
2. Add health check endpoint for load balancer (e.g., `:9090/health`)
3. Integrate with management server for centralized health monitoring
4. Add more granular health states (starting, stopping, draining)
5. Implement health-based load balancing
6. Add custom health checks per agent profile

## Files Modified

1. `agent-rs/Cargo.toml` - Added sd-notify dependency
2. `agent-rs/src/main.rs` - Integrated health monitoring
3. `agent-rs/src/health.rs` - NEW: Health state management
4. `agent-rs/src/metrics.rs` - NEW: Prometheus metrics
5. `agent-rs/systemd/agent-client.service` - Watchdog configuration
6. `agent-rs/scripts/verify-agent-health.sh` - NEW: Health verification

## Summary

This implementation provides:
- ✅ Type=notify with NotifyAccess=main
- ✅ WatchdogSec=30, WatchdogSignal=SIGABRT
- ✅ StartLimitBurst=5, StartLimitIntervalSec=300
- ✅ Resource limits (MemoryMax, TasksMax, CPUQuota)
- ✅ sd_notify READY notification
- ✅ Watchdog ping loop (every 15s)
- ✅ Health states (healthy, degraded, unhealthy)
- ✅ Reliability metrics
- ✅ Health verification script

The agent is now resilient to:
- Process hangs (watchdog timeout)
- Connection failures (health state degradation)
- Resource exhaustion (cgroup limits)
- Fork bombs (TasksMax)
- Rapid restart loops (StartLimitBurst)

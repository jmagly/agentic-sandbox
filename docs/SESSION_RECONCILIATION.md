# Session Reconciliation Protocol Design

**Issue:** #99 - Agent should clean up orphaned PTY sessions on reconnect
**Status:** Draft
**Author:** Architecture Designer
**Date:** 2026-02-02

## Problem Statement

When the management server restarts, it loses track of active PTY sessions on agents. The agents continue running PTY processes (typically tmux sessions) that:

1. Send output for command IDs the server no longer recognizes
2. Cause terminal corruption when users reconnect to a "new" session
3. May leak resources (orphaned tmux/PTY processes) over time

### Current Behavior

```
Agent                             Server (restarted)
  |                                    |
  |-------- stdout for cmd-abc ------->| "unknown command: cmd-abc" (warning)
  |                                    |
  |<------ new shell: cmd-xyz ---------|
  |                                    |
  |  (tmux "main" still running        |
  |   from old cmd-abc, conflicting    |
  |   with new cmd-xyz for "main")     |
```

### Desired Behavior

```
Agent                             Server (restarted)
  |                                    |
  |-------- Registration ------------->|
  |<------- RegistrationAck -----------|
  |                                    |
  |<------- SessionQuery --------------|
  |                                    |
  |-------- SessionReport ------------>| (reports cmd-abc active)
  |                                    |
  |<------- SessionReconcile ----------| (kill cmd-abc, unknown to server)
  |                                    |
  |<------ new shell: cmd-xyz ---------|
  |                                    |
  |  (clean slate, no conflicts)       |
```

---

## Design Decision

### Recommendation: Hybrid Approach (Server-Initiated with Agent-Reported State)

After analyzing the options, I recommend a **hybrid approach** that combines the strengths of server-initiated and agent-initiated reconciliation:

| Aspect | Server-Initiated | Agent-Initiated | **Hybrid (Recommended)** |
|--------|------------------|-----------------|--------------------------|
| Complexity | Low | High | Medium |
| Race conditions | Few | Many | Few |
| Recovery from partial failures | Limited | Better | Best |
| Preserves sessions on brief network issues | No (kill-all) | Yes | Yes |
| Server is authoritative | Yes | No | Yes |

### Why Hybrid?

1. **Server is authoritative** - Only the server knows which sessions are valid
2. **Agent has ground truth** - Only the agent knows what's actually running
3. **Graceful reconciliation** - Preserves sessions that the server still recognizes
4. **Handles all edge cases** - Both reconnect and server restart scenarios

---

## Protocol Design

### New gRPC Messages

Add to `proto/agent.proto`:

```protobuf
// =============================================================================
// Session Reconciliation Messages
// =============================================================================

// Server queries agent for active sessions
message SessionQuery {
  // If true, agent should report all active sessions
  // If false, agent should only report sessions matching session_ids
  bool report_all = 1;

  // Optional: specific session IDs to query (if report_all is false)
  repeated string session_ids = 2;
}

// Agent reports its active sessions to server
message SessionReport {
  string agent_id = 1;
  repeated ActiveSession sessions = 2;
  int64 timestamp_ms = 3;
}

// Description of an active session on the agent
message ActiveSession {
  string command_id = 1;        // UUID assigned by server
  string session_name = 2;      // e.g., "main", "claude"
  SessionType session_type = 3; // Interactive, Headless, Background
  string command = 4;           // Original command string
  int64 started_at_ms = 5;      // When session was created
  int32 pid = 6;                // Process ID (for debugging)
  bool is_pty = 7;              // Whether this is a PTY session
}

// Session types matching dispatcher.rs
enum SessionType {
  SESSION_TYPE_UNKNOWN = 0;
  SESSION_TYPE_INTERACTIVE = 1;
  SESSION_TYPE_HEADLESS = 2;
  SESSION_TYPE_BACKGROUND = 3;
}

// Server instructs agent to reconcile sessions
message SessionReconcile {
  // Sessions to keep (server recognizes these)
  repeated string keep_session_ids = 1;

  // Sessions to terminate (server doesn't recognize)
  repeated string kill_session_ids = 2;

  // Kill all sessions not in keep_session_ids (nuclear option)
  bool kill_unrecognized = 3;

  // Grace period before SIGKILL (seconds)
  int32 grace_period_seconds = 4;
}

// Agent confirms reconciliation complete
message SessionReconcileAck {
  string agent_id = 1;
  repeated string killed_session_ids = 2;
  repeated string kept_session_ids = 3;
  repeated string failed_to_kill = 4;  // Sessions that couldn't be terminated
  int64 timestamp_ms = 5;
}
```

### Updated Message Envelopes

Add to `AgentMessage.payload` oneof:

```protobuf
message AgentMessage {
  oneof payload {
    // ... existing fields ...
    SessionReport session_report = 8;
    SessionReconcileAck session_reconcile_ack = 9;
  }
}
```

Add to `ManagementMessage.payload` oneof:

```protobuf
message ManagementMessage {
  oneof payload {
    // ... existing fields ...
    SessionQuery session_query = 8;
    SessionReconcile session_reconcile = 9;
  }
}
```

---

## Sequence Diagrams

### Normal Agent Reconnection (Server Running)

```
Agent                                 Server
  |                                      |
  |--- Connect (stream open) ----------->|
  |                                      |
  |--- Registration --------------------->|
  |                                      |
  |<-- RegistrationAck -------------------|
  |                                      |
  |<-- SessionQuery (report_all=true) ----|
  |                                      |
  |--- SessionReport (sessions: [        |
  |      {cmd-abc, "main", Interactive}, |
  |      {cmd-def, "claude", Headless}   |
  |    ]) ------------------------------>|
  |                                      |
  |    [Server checks dispatcher:        |
  |     - cmd-abc: NOT in active_sessions|
  |     - cmd-def: in active_sessions]   |
  |                                      |
  |<-- SessionReconcile (               |
  |      kill: [cmd-abc],               |
  |      keep: [cmd-def]                |
  |    ) --------------------------------|
  |                                      |
  |    [Agent kills cmd-abc PTY]         |
  |                                      |
  |--- SessionReconcileAck (            |
  |      killed: [cmd-abc],             |
  |      kept: [cmd-def]                |
  |    ) ------------------------------->|
  |                                      |
  |    [Reconciliation complete]         |
```

### Server Restart (Agent Already Connected)

```
Agent                                 Server (new instance)
  |                                      |
  |    [gRPC stream broken]              |
  |                                      |
  |--- Connect (reconnect) -------------->|
  |                                      |
  |--- Registration --------------------->|
  |                                      |
  |<-- RegistrationAck -------------------|
  |                                      |
  |<-- SessionQuery (report_all=true) ----|
  |                                      |
  |--- SessionReport (sessions: [        |
  |      {cmd-old1, "main", Interactive},|
  |      {cmd-old2, "debug", Interactive}|
  |    ]) ------------------------------>|
  |                                      |
  |    [Server has empty dispatcher:     |
  |     NO sessions recognized]          |
  |                                      |
  |<-- SessionReconcile (               |
  |      kill_unrecognized=true,        |
  |      keep: []                       |
  |    ) --------------------------------|
  |                                      |
  |    [Agent kills ALL sessions]        |
  |                                      |
  |--- SessionReconcileAck (            |
  |      killed: [cmd-old1, cmd-old2]   |
  |    ) ------------------------------->|
  |                                      |
  |    [Clean slate achieved]            |
```

### Brief Network Interruption (Graceful Recovery)

```
Agent                                 Server
  |                                      |
  |    [Network blip, 5 second outage]   |
  |                                      |
  |--- Connect (reconnect) -------------->|
  |                                      |
  |--- Registration --------------------->|
  |                                      |
  |<-- RegistrationAck -------------------|
  |                                      |
  |<-- SessionQuery (report_all=true) ----|
  |                                      |
  |--- SessionReport (sessions: [        |
  |      {cmd-xyz, "main", Interactive}  |
  |    ]) ------------------------------>|
  |                                      |
  |    [Server checks dispatcher:        |
  |     - cmd-xyz: STILL in pending      |
  |       (not yet cleaned up)]          |
  |                                      |
  |<-- SessionReconcile (               |
  |      keep: [cmd-xyz]                |
  |    ) --------------------------------|
  |                                      |
  |--- SessionReconcileAck (            |
  |      kept: [cmd-xyz]                |
  |    ) ------------------------------->|
  |                                      |
  |    [Session preserved!]              |
```

---

## Implementation Details

### Server-Side Changes

#### 1. Dispatcher Enhancement (`management/src/dispatch/dispatcher.rs`)

```rust
impl CommandDispatcher {
    /// Get all known command IDs for an agent
    pub fn get_known_command_ids(&self, agent_id: &str) -> Vec<String> {
        let mut ids = Vec::new();

        // From pending commands
        for entry in self.pending.read().iter() {
            if entry.1.agent_id == agent_id {
                ids.push(entry.0.clone());
            }
        }

        // From active sessions
        if let Some(sessions) = self.active_sessions.read().get(agent_id) {
            for info in sessions.values() {
                if !ids.contains(&info.command_id) {
                    ids.push(info.command_id.clone());
                }
            }
        }

        ids
    }

    /// Reconcile agent sessions after reconnect
    pub async fn reconcile_sessions(
        &self,
        agent_id: &str,
        reported_sessions: &[ActiveSession],
    ) -> SessionReconcile {
        let known_ids = self.get_known_command_ids(agent_id);

        let mut keep = Vec::new();
        let mut kill = Vec::new();

        for session in reported_sessions {
            if known_ids.contains(&session.command_id) {
                keep.push(session.command_id.clone());
            } else {
                kill.push(session.command_id.clone());
            }
        }

        SessionReconcile {
            keep_session_ids: keep,
            kill_session_ids: kill,
            kill_unrecognized: false,  // Explicit list preferred
            grace_period_seconds: 5,
        }
    }

    /// Handle reconciliation acknowledgment
    pub fn handle_reconcile_ack(&self, agent_id: &str, ack: SessionReconcileAck) {
        // Remove killed sessions from our tracking
        for killed_id in &ack.killed_session_ids {
            self.pending.write().remove(killed_id);
        }

        // Update active_sessions tracking
        if let Some(mut sessions) = self.active_sessions.write().get_mut(agent_id) {
            sessions.retain(|_, info| !ack.killed_session_ids.contains(&info.command_id));
        }

        info!(
            agent_id = %agent_id,
            killed = ?ack.killed_session_ids,
            kept = ?ack.kept_session_ids,
            failed = ?ack.failed_to_kill,
            "Session reconciliation complete"
        );
    }
}
```

#### 2. gRPC Handler Enhancement (`management/src/grpc.rs`)

```rust
async fn handle_agent_message(
    registry: &Arc<AgentRegistry>,
    dispatcher: &Arc<CommandDispatcher>,
    output_agg: &Arc<OutputAggregator>,
    agent_id: &str,
    msg: AgentMessage,
    tx: mpsc::Sender<ManagementMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match msg.payload {
        Some(agent_message::Payload::Registration(reg)) => {
            // ... existing registration handling ...

            // Trigger session reconciliation after registration
            let query = SessionQuery {
                report_all: true,
                session_ids: vec![],
            };
            tx.send(ManagementMessage {
                payload: Some(management_message::Payload::SessionQuery(query)),
            }).await?;
        }

        Some(agent_message::Payload::SessionReport(report)) => {
            info!(
                agent_id = %agent_id,
                session_count = report.sessions.len(),
                "Received session report"
            );

            // Generate reconciliation instruction
            let reconcile = dispatcher.reconcile_sessions(agent_id, &report.sessions).await;

            tx.send(ManagementMessage {
                payload: Some(management_message::Payload::SessionReconcile(reconcile)),
            }).await?;
        }

        Some(agent_message::Payload::SessionReconcileAck(ack)) => {
            dispatcher.handle_reconcile_ack(agent_id, ack);
        }

        // ... existing handlers ...
    }
}
```

### Agent-Side Changes

#### 1. Session Tracking (`agent-rs/src/main.rs`)

```rust
impl AgentClient {
    /// Build session report from running_commands
    async fn build_session_report(&self) -> SessionReport {
        let running = self.running_commands.lock().await;

        let sessions: Vec<ActiveSession> = running
            .iter()
            .map(|(cmd_id, cmd)| ActiveSession {
                command_id: cmd_id.clone(),
                session_name: cmd.session_name.clone().unwrap_or_default(),
                session_type: cmd.session_type.into(),
                command: cmd.command.clone(),
                started_at_ms: cmd.started_at.elapsed().as_millis() as i64,
                pid: cmd.pid.map(|p| p.as_raw()).unwrap_or(0),
                is_pty: cmd.pty_control_tx.is_some(),
            })
            .collect();

        SessionReport {
            agent_id: self.config.agent_id.clone(),
            sessions,
            timestamp_ms: chrono_timestamp_ms(),
        }
    }

    /// Kill sessions as instructed by server
    async fn kill_sessions(&self, session_ids: &[String], grace_seconds: i32) {
        for cmd_id in session_ids {
            if let Some(cmd) = self.running_commands.lock().await.get(cmd_id) {
                // Send SIGTERM first
                if let Some(pid) = cmd.pid {
                    let _ = nix::sys::signal::kill(pid, Signal::SIGTERM);
                }
            }
        }

        // Wait for grace period
        tokio::time::sleep(Duration::from_secs(grace_seconds as u64)).await;

        // SIGKILL any survivors
        for cmd_id in session_ids {
            if let Some(cmd) = self.running_commands.lock().await.remove(cmd_id) {
                if let Some(pid) = cmd.pid {
                    let _ = nix::sys::signal::kill(pid, Signal::SIGKILL);
                }
            }
        }
    }
}
```

#### 2. Handle Reconciliation Messages

```rust
async fn handle_inbound(&self, msg: ManagementMessage, output_tx: mpsc::Sender<AgentMessage>) {
    match msg.payload {
        // ... existing handlers ...

        Some(Payload::SessionQuery(query)) => {
            info!("Received session query (report_all={})", query.report_all);

            let report = self.build_session_report().await;
            let _ = output_tx.send(AgentMessage {
                payload: Some(agent_message::Payload::SessionReport(report)),
            }).await;
        }

        Some(Payload::SessionReconcile(reconcile)) => {
            info!(
                "Received session reconcile: keep={}, kill={}",
                reconcile.keep_session_ids.len(),
                reconcile.kill_session_ids.len()
            );

            let to_kill = if reconcile.kill_unrecognized {
                // Kill everything not in keep list
                let running = self.running_commands.lock().await;
                running.keys()
                    .filter(|id| !reconcile.keep_session_ids.contains(id))
                    .cloned()
                    .collect()
            } else {
                reconcile.kill_session_ids.clone()
            };

            self.kill_sessions(&to_kill, reconcile.grace_period_seconds).await;

            // Build acknowledgment
            let kept: Vec<String> = self.running_commands.lock().await.keys().cloned().collect();
            let ack = SessionReconcileAck {
                agent_id: self.config.agent_id.clone(),
                killed_session_ids: to_kill,
                kept_session_ids: kept,
                failed_to_kill: vec![],  // TODO: track actual failures
                timestamp_ms: chrono_timestamp_ms(),
            };

            let _ = output_tx.send(AgentMessage {
                payload: Some(agent_message::Payload::SessionReconcileAck(ack)),
            }).await;
        }
    }
}
```

---

## Edge Cases

### 1. Race: Command Arrives During Reconciliation

**Scenario:** Server sends new command while agent is processing SessionReconcile.

**Mitigation:**
- Agent processes messages sequentially on the inbound stream
- New commands queued after reconciliation completes
- Server waits for ReconcileAck before dispatching new sessions

### 2. Agent Crashes Mid-Reconciliation

**Scenario:** Agent dies after receiving SessionReconcile but before sending Ack.

**Mitigation:**
- Server times out waiting for Ack (30s default)
- Server re-queries on next agent reconnect
- Sessions may be orphaned until next reconnect, but not duplicated

### 3. Server Crashes Mid-Reconciliation

**Scenario:** Server dies after sending SessionQuery but before receiving Report.

**Mitigation:**
- Agent's SessionReport is lost to the void
- On new server startup, agent reconnects and process repeats
- No harm done; agent state unchanged

### 4. Partial Kill Failure

**Scenario:** Some sessions fail to terminate (zombie processes, etc.)

**Mitigation:**
- Agent reports `failed_to_kill` list in Ack
- Server logs warning and may retry with SIGKILL
- Operator can manually investigate via SSH

### 5. tmux Session vs PTY Process Mismatch

**Scenario:** tmux session exists but tracked PTY was killed.

**Mitigation:**
- Agent tracks both PTY process PID and tmux session name
- Kill tmux session by name (`tmux kill-session -t <name>`) as backup
- Report includes both for debugging

### 6. Multiple Agents with Same Session Names

**Scenario:** agent-01 and agent-02 both have "main" session.

**Mitigation:**
- Sessions tracked per-agent in dispatcher (`agent_id -> session_name -> info`)
- Reconciliation scoped to single agent
- No cross-agent interference

---

## Metrics and Observability

### Prometheus Metrics

```
# Session reconciliation counts
agentic_session_reconciliations_total{agent_id, result="success|partial|failed"}

# Sessions killed during reconciliation
agentic_sessions_killed_total{agent_id, reason="orphaned|server_restart|manual"}

# Sessions preserved during reconciliation
agentic_sessions_preserved_total{agent_id}

# Reconciliation latency
agentic_reconciliation_duration_seconds{agent_id}
```

### Structured Logging

```json
{
  "timestamp": "2026-02-02T10:00:00Z",
  "level": "info",
  "event": "session_reconciliation_complete",
  "agent_id": "agent-01",
  "sessions_killed": 2,
  "sessions_preserved": 1,
  "sessions_failed": 0,
  "duration_ms": 1234
}
```

---

## Testing Strategy

### Unit Tests

1. `test_session_report_building` - Verify agent builds correct report from running_commands
2. `test_reconcile_decision_logic` - Server correctly identifies orphaned vs valid sessions
3. `test_kill_signal_sequence` - SIGTERM followed by SIGKILL after grace period

### Integration Tests

1. `test_reconnection_preserves_valid_sessions` - Session survives brief disconnect
2. `test_server_restart_cleans_orphans` - All sessions killed after server restart
3. `test_partial_kill_failure_handling` - Graceful handling of unkillable processes

### E2E Tests

1. `test_e2e_server_restart_recovery` - Full server restart with active terminal session
2. `test_e2e_network_partition_recovery` - Simulated network outage with session preservation

---

## Rollout Plan

### Phase 1: Protocol Implementation (Non-Breaking)

1. Add new protobuf messages (backward compatible)
2. Implement server-side reconciliation logic
3. Server sends SessionQuery but handles missing response gracefully

### Phase 2: Agent Implementation

1. Agent responds to SessionQuery with SessionReport
2. Agent handles SessionReconcile instructions
3. Feature flag: `ENABLE_SESSION_RECONCILIATION=true`

### Phase 3: Full Enablement

1. Enable by default
2. Monitor metrics for reconciliation success rate
3. Tune grace periods based on observed behavior

---

## Appendix A: Alternative Designs Considered

### Option 1: Server-Initiated Kill-All

Simple approach where server sends "kill all sessions" on agent reconnect.

**Pros:** Simple, no new protocol needed
**Cons:** Destroys valid sessions on brief network issues

### Option 2: Agent-Initiated Cleanup

Agent detects server restart and cleans up autonomously.

**Pros:** Agent is self-healing
**Cons:** Agent can't know which sessions server still expects; race conditions

### Option 3: Session Persistence to Disk

Server persists session state to disk, recovers on restart.

**Pros:** Sessions survive server restart
**Cons:** Complex persistence layer; state drift between disk and agent

---

## Appendix B: Related Issues

- Issue #99: Agent should clean up orphaned PTY sessions on reconnect (this design)
- Issue #XX: Session persistence across server restarts (future enhancement)
- Issue #XX: Multi-agent session handoff (future enhancement)

# Session & Process Invocation Architecture

This document describes the process invocation and session management architecture for agentic-sandbox.

## Overview

The agentic-sandbox provides process execution and session management spanning
agent VMs, management server, and Web UI. Release-specific security boundaries
are tracked in [Security Status](security/security-status.md).

### Key Characteristics

- **Bidirectional gRPC streaming** for command dispatch and output collection
- **PTY-enabled interactive sessions** with terminal resizing and signal control
- **tmux integration** for session persistence across reconnects
- **Task orchestration** with full lifecycle management
- **Real-time streaming** via WebSocket. The gRPC PTY path is expected to be
  lighter than SSH cold sessions, but launch-facing performance language should
  cite the qualified benchmark artifact in
  [terminal-transport-benchmark-2026-06-19.md](../.aiwg/testing/terminal-transport-benchmark-2026-06-19.md)
  rather than making an unqualified claim.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              INVOCATION LAYER                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │  REST API    │   │  WebSocket   │   │ Task Submit  │   │   CLI        │ │
│  │  /api/v1/    │   │  Terminal    │   │ Orchestrator │   │  (future)    │ │
│  └──────┬───────┘   └──────┬───────┘   └──────┬───────┘   └──────┬───────┘ │
│         │                  │                  │                  │          │
│         └──────────────────┴────────┬─────────┴──────────────────┘          │
│                                     │                                        │
│                          ┌──────────▼──────────┐                            │
│                          │  Command Dispatcher  │                            │
│                          │  (dispatch.rs)       │                            │
│                          └──────────┬──────────┘                            │
│                                     │                                        │
├─────────────────────────────────────┼────────────────────────────────────────┤
│                              TRANSPORT                                       │
│                                     │                                        │
│                          ┌──────────▼──────────┐                            │
│                          │   gRPC BiDi Stream  │                            │
│                          │   (grpc.rs)         │                            │
│                          └──────────┬──────────┘                            │
│                                     │                                        │
├─────────────────────────────────────┼────────────────────────────────────────┤
│                              AGENT EXECUTION                                 │
│                                     │                                        │
│              ┌──────────────────────┼───────────────────────┐               │
│              │                      │                       │               │
│    ┌─────────▼─────────┐  ┌─────────▼─────────┐  ┌─────────▼─────────┐     │
│    │  execute_command  │  │ execute_cmd_pty   │  │ execute_claude    │     │
│    │  (standard)       │  │ (interactive)     │  │ (headless agent)  │     │
│    │  - piped I/O      │  │ - PTY master/slave│  │ - stream-json     │     │
│    │  - timeout        │  │ - fork/exec       │  │ - event parsing   │     │
│    │  - stdin support  │  │ - resize/signal   │  │ - tool tracking   │     │
│    └───────────────────┘  └───────────────────┘  └───────────────────┘     │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Session Types

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           SESSION TYPES                                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐             │
│  │  INTERACTIVE    │  │   HEADLESS      │  │   BACKGROUND    │             │
│  │                 │  │                 │  │                 │             │
│  │  User controls  │  │  Automated      │  │  Long-running   │             │
│  │  via WebSocket  │  │  Claude agent   │  │  detached       │             │
│  │  terminal       │  │  with prompt    │  │  process        │             │
│  │                 │  │                 │  │                 │             │
│  │  PTY: yes       │  │  PTY: optional  │  │  PTY: optional  │             │
│  │  stdin: yes     │  │  stdin: no      │  │  stdin: fifo    │             │
│  │  persist: tmux  │  │  persist: tmux  │  │  persist: tmux  │             │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘             │
│                                                                              │
│  Examples:                                                                   │
│  - User debugging    - /ralph task      - npm run dev                       │
│  - Manual claude     - CI pipeline      - docker compose up                 │
│  - SSH-like session  - Batch process    - Long compilation                  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## tmux Integration

Interactive shells use tmux for session persistence:

```rust
// dispatcher.rs:331-340
command: "tmux".to_string(),
args: vec![
    "new-session".to_string(),
    "-A".to_string(),  // Attach if exists, create if not
    "-s".to_string(),
    "main".to_string(),
],
```

Benefits:
- Sessions survive WebSocket disconnect
- Users can detach and reattach
- Reconnection avoids a new SSH cold setup; see the qualified benchmark artifact
  in
  [terminal-transport-benchmark-2026-06-19.md](../.aiwg/testing/terminal-transport-benchmark-2026-06-19.md)
- Terminal state preserved

```
┌──────────────────────────────────────────────────────────────────┐
│                    tmux SESSION ARCHITECTURE                      │
├──────────────────────────────────────────────────────────────────┤
│                                                                   │
│   Agent Process                                                   │
│   ┌───────────────────────────────────────────────────────────┐  │
│   │  Agent Client (gRPC)                                       │  │
│   │                                                            │  │
│   │  Running Commands:                                         │  │
│   │    "cmd-abc" → PTY master fd, stdin_tx, pty_control_tx    │  │
│   │    "cmd-def" → PTY master fd, stdin_tx, pty_control_tx    │  │
│   │                                                            │  │
│   └───────────────────────────────────────────────────────────┘  │
│                            │                                      │
│                            ▼                                      │
│   ┌───────────────────────────────────────────────────────────┐  │
│   │  tmux server (persistent)                                  │  │
│   │                                                            │  │
│   │  Sessions:                                                 │  │
│   │    0: main        (1 window, may be attached/detached)    │  │
│   │    1: claude-task (1 window, headless agent)              │  │
│   │    2: background  (1 window, long-running process)        │  │
│   │                                                            │  │
│   └───────────────────────────────────────────────────────────┘  │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
```

## Process Execution Flow

### Flow A: User Executes Command via WebSocket

```
WS Client
  ↓ {type: "send_command", agent_id: "agent-01", command: "ls", args: ["-la"]}
  ↓
ws::connection.rs handle_client_message()
  ↓ Dispatch to dispatcher
  ↓
CommandDispatcher::dispatch()
  ↓ Generate command_id, create output channel, store in pending HashMap
  ↓ Build CommandRequest with allocate_pty=false
  ↓
AgentRegistry::send_command()
  ↓ Send via mpsc to agent's command_tx
  ↓
Agent (gRPC stream)
  ↓ Receive ManagementMessage::Command
  ↓
handle_inbound() routes to execute_command()
  ↓
tokio::process::Command::spawn()
  ↓
stdout/stderr streamed back as OutputChunk
  ↓
Output handling in grpc.rs: handle_stdout()
  ↓
OutputAggregator + CommandDispatcher output_tx
  ↓
OutputChunk forwarded to WS subscribers
```

### Flow B: User Opens Interactive Shell via WebSocket

```
WS Client
  ↓ {type: "start_shell", agent_id: "agent-01", cols: 120, rows: 40}
  ↓
ws::connection.rs → CommandDispatcher::dispatch_shell()
  ↓
CommandRequest with:
  - command: "tmux"
  - args: ["new-session", "-A", "-s", "main"]
  - allocate_pty: true
  - pty_cols: 120, pty_rows: 40
  ↓
Agent receives Command message
  ↓
execute_command_pty() dispatched
  ↓
openpty() → fork() → child: setsid(), dup2, execvp(tmux)
  ↓
Parent: blocking read on master fd, stdin written to master fd
  ↓
PtyControl messages handle:
  - PtyResize: SIGWINCH to child
  - PtySignal: arbitrary signals
  ↓
Bidirectional I/O until detach or exit
  ↓
tmux session persists even after WebSocket disconnect
```

### Flow C: Task Orchestration (Claude Execution)

```
REST Client (curl / UI)
  ↓ POST /api/v1/tasks
  ↓ { manifest_yaml: "..." }
  ↓
http::tasks.rs submit_task()
  ↓ Parse manifest → TaskManifest::from_yaml()
  ↓ Generate task_id
  ↓
Orchestrator::submit_task()
  ↓ Validate manifest
  ↓ Create Task (state=Pending)
  ↓ Save checkpoint
  ↓ Spawn background task lifecycle execution
  ↓
[STAGING]
  ↓ TaskExecutor::stage_task()
  ↓ Git clone repo → inbox
  ↓ Write TASK.md with prompt
  ↓
[PROVISIONING]
  ↓ TaskExecutor::provision_vm()
  ↓ Run provision-vm.sh script
  ↓ Extract VM IP from vm-info.json
  ↓
[READY]
  ↓ Wait for agent registration via gRPC
  ↓ Agent connects: Registration → SHA256 verification ✓
  ↓
[RUNNING]
  ↓ TaskExecutor::execute_claude()
  ↓ Send __claude_task__ command via gRPC
  ↓
Agent receives command
  ↓ execute_claude_task()
  ↓ Spawn: Command::new("claude") with stream-json
  ↓ Parse events, forward output
  ↓
[COMPLETING]
  ↓ Collect artifacts from outbox
  ↓
[COMPLETED]
  ↓ Save exit code, cleanup VM
```

## Collaborative Mode

Support manual + automated sessions simultaneously:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     COLLABORATIVE SESSION EXAMPLE                            │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Agent VM: agent-01                                                          │
│                                                                              │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  tmux sessions:                                                      │   │
│  │                                                                      │   │
│  │  Session "main" (Interactive - User Attached)                       │   │
│  │  ┌────────────────────────────────────────────────────────────────┐ │   │
│  │  │ agent@agent-01:~/project$ vim src/main.rs                      │ │   │
│  │  │ ~                                                               │ │   │
│  │  │ User editing code while agent works in background              │ │   │
│  │  └────────────────────────────────────────────────────────────────┘ │   │
│  │                                                                      │   │
│  │  Session "claude-task-abc" (Headless Claude - Background)           │   │
│  │  ┌────────────────────────────────────────────────────────────────┐ │   │
│  │  │ [claude] Analyzing test failures...                            │ │   │
│  │  │ [claude] Running: cargo test --lib                             │ │   │
│  │  │ [claude] Found issue in auth module, fixing...                 │ │   │
│  │  │ Streaming output visible to user via WebSocket subscription    │ │   │
│  │  └────────────────────────────────────────────────────────────────┘ │   │
│  │                                                                      │   │
│  │  Session "dev-server" (Background - npm)                            │   │
│  │  ┌────────────────────────────────────────────────────────────────┐ │   │
│  │  │ > ui@0.1.0 dev                                                 │ │   │
│  │  │ > vite                                                          │ │   │
│  │  │ VITE v5.0.0 ready in 234ms                                     │ │   │
│  │  │ ➜ Local: http://localhost:5173/                                │ │   │
│  │  └────────────────────────────────────────────────────────────────┘ │   │
│  │                                                                      │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                              │
│  UI Dashboard shows:                                                        │
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │  agent-01 Sessions:                                                   │  │
│  │  [●] main (attached)         - "interactive shell"    [Detach]       │  │
│  │  [○] claude-task-abc (run)   - "fix test failures"    [View Log]     │  │
│  │  [○] dev-server (running)    - "npm run dev"          [View Log]     │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Agent Connection Lifecycle

```
Agent VM boots
  ↓
Agent client reads /etc/agentic-sandbox/agent.env (cloud-init injected)
  ↓ AGENT_ID, MANAGEMENT_SERVER, secure transport material
  ↓
AgentClient::connect() establishes gRPC connection to management
  ↓
Agent sends AgentRegistration (ID, IP, hostname, system_info)
  ↓
Management verifies SHA256(secret) against stored hash
  ↓
Send RegistrationAck (heartbeat interval, config)
  ↓
Heartbeat task spawned: every 5 seconds send Heartbeat + Metrics
  ↓
Forward output_rx channel messages on stream
  ↓
While connected: handle inbound ManagementMessages
  ↓
On disconnect / reconnect: exponential backoff (5s → 60s)
  ↓
tmux sessions persist across reconnects
```

## Command Execution Session Lifecycle

```
receive CommandRequest
  ↓
create RunningCommand entry (PID, stdin_tx, pty_control_tx)
  ↓
spawn execution task (or PTY fork)
  ↓
while executing:
  - collect output chunks
  - handle stdin updates
  - handle PTY control (resize/signal)
  ↓
process exits (natural or timeout)
  ↓
send CommandResult (exit_code, duration_ms)
  ↓
remove RunningCommand entry
  ↓
send OutputChunk with eof=true
  ↓
tmux session may continue running (detached)
```

## Key Files Reference

| Component | File | Purpose |
|-----------|------|---------|
| Agent Command Execution | agent-rs/src/main.rs | Command routing, execution (std/PTY/Claude) |
| gRPC Service | management/src/grpc.rs | Agent connection, message handling |
| Command Dispatcher | management/src/dispatch/dispatcher.rs | Command tracking, output routing, tmux shell |
| Agent Registry | management/src/registry.rs | Connected agents, channels |
| Task Orchestrator | management/src/orchestrator/mod.rs | Task lifecycle, execution flow |
| WebSocket Handler | management/src/ws/connection.rs | Real-time streaming |
| HTTP API | management/src/http/tasks.rs | REST endpoints |
| Proto Definitions | proto/agent.proto | All message types |

## Proto Message Summary

| Message | Purpose |
|---------|---------|
| CommandRequest | Execute command (with PTY options) |
| OutputChunk | Stream stdout/stderr data |
| CommandResult | Completion status |
| StdinChunk | Send input to process |
| PtyControl | Resize terminal, send signals |
| Heartbeat | Agent liveness + basic metrics |
| Metrics | Full system metrics |

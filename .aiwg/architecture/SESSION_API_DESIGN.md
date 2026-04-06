# Session Management API Design

## Overview

This document defines the Session Management API for agentic-sandbox. Sessions allow users to create, attach to, detach from, and manage multiple named terminal and process sessions on agent VMs, similar to `tmux` or `screen`.

## Rationale

The current system dispatches commands one-at-a-time through the CommandDispatcher. For persistent, interactive workflows, we need:

1. **Named Sessions**: Create reusable sessions by name (e.g., "dev-shell", "build-watch")
2. **Attach/Detach**: Disconnect from a session without killing it, reconnect later
3. **Multiple Session Types**: Support interactive terminals, headless agents, and background processes
4. **Session Persistence**: Sessions survive WebSocket disconnections
5. **Session Listing**: View all active sessions per agent

## Session Types

```rust
/// Type of session execution mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionType {
    /// Interactive PTY terminal (user-controlled shell)
    Interactive,

    /// Headless Claude Code agent (automated AI session)
    Headless,

    /// Background long-running process (detached daemon)
    Background,
}

/// Session metadata and state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Unique session ID (UUID)
    pub session_id: String,

    /// Human-readable session name (unique per agent)
    pub session_name: String,

    /// Session execution mode
    pub session_type: SessionType,

    /// Agent ID hosting this session
    pub agent_id: String,

    /// Command executed in session
    pub command: String,

    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,

    /// Working directory
    #[serde(default)]
    pub working_dir: String,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Session creation timestamp (Unix milliseconds)
    pub created_at: i64,

    /// Whether session is currently attached
    pub attached: bool,

    /// Whether process is still running
    pub running: bool,

    /// Exit code if process completed
    pub exit_code: Option<i32>,

    /// PTY dimensions (if interactive)
    pub pty_cols: Option<u32>,
    pub pty_rows: Option<u32>,
}
```

## WebSocket API

### Client Messages

These messages extend `ClientMessage` in `management/src/ws/connection.rs`:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    // ... existing messages ...

    /// Create a new named session
    CreateSession {
        agent_id: String,
        session_name: String,
        session_type: SessionType,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        working_dir: String,
        #[serde(default)]
        env: HashMap<String, String>,
        /// PTY dimensions (required for Interactive type)
        #[serde(default = "default_cols")]
        cols: u32,
        #[serde(default = "default_rows")]
        rows: u32,
    },

    /// List all sessions for an agent
    ListSessions {
        agent_id: String,
    },

    /// Attach to an existing session (resume output streaming)
    AttachSession {
        agent_id: String,
        session_id: String,
        /// PTY dimensions (for resizing on attach if session is Interactive)
        #[serde(default = "default_cols")]
        cols: u32,
        #[serde(default = "default_rows")]
        rows: u32,
    },

    /// Detach from session (stop output streaming, keep process alive)
    DetachSession {
        agent_id: String,
        session_id: String,
    },

    /// Kill session (terminate process and cleanup)
    KillSession {
        agent_id: String,
        session_id: String,
        /// Signal to send (default: SIGTERM=15)
        #[serde(default = "default_signal")]
        signal: i32,
    },

    /// Send input to attached session (replaces SendInput with session awareness)
    SendSessionInput {
        agent_id: String,
        session_id: String,
        data: String,
    },
}

fn default_cols() -> u32 { 80 }
fn default_rows() -> u32 { 24 }
fn default_signal() -> i32 { 15 } // SIGTERM
```

### Server Messages

These messages extend `ServerMessage` in `management/src/ws/connection.rs`:

```rust
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    // ... existing messages ...

    /// Session created successfully
    SessionCreated {
        agent_id: String,
        session_id: String,
        session_name: String,
        session_type: SessionType,
    },

    /// List of sessions for an agent
    SessionList {
        agent_id: String,
        sessions: Vec<SessionInfo>,
    },

    /// Session attached
    SessionAttached {
        agent_id: String,
        session_id: String,
        session_name: String,
    },

    /// Session detached
    SessionDetached {
        agent_id: String,
        session_id: String,
    },

    /// Session killed/terminated
    SessionKilled {
        agent_id: String,
        session_id: String,
        exit_code: Option<i32>,
    },

    /// Session state changed (running -> exited, etc.)
    SessionStateChanged {
        agent_id: String,
        session_id: String,
        running: bool,
        exit_code: Option<i32>,
    },

    /// Session output (replaces Output with session awareness)
    SessionOutput {
        agent_id: String,
        session_id: String,
        session_name: String,
        stream: String, // "stdout" | "stderr"
        data: String,
        ts: i64,
    },
}
```

### WebSocket Message Flow Diagrams

#### Creating and Attaching to a Session

```
Client                          Management Server                 Agent VM
  |                                     |                            |
  |--CreateSession------------------>  |                            |
  |  {session_name: "dev-shell"}       |                            |
  |                                     |--CommandRequest---------->|
  |                                     |  (with session metadata)   |
  |                                     |                            |
  |<-SessionCreated-------------------|                            |
  |  {session_id: "abc123"}            |                            |
  |                                     |                            |
  |  (Output automatically streams)    |<-OutputChunk--------------|
  |<-SessionOutput--------------------|                            |
  |  {data: "$ "}                      |                            |
  |                                     |                            |
  |--SendSessionInput---------------->|                            |
  |  {data: "ls -la\n"}                |--StdinChunk-------------->|
  |                                     |                            |
  |<-SessionOutput--------------------|<-OutputChunk--------------|
  |  {data: "total 48..."}             |                            |
```

#### Detaching and Reattaching

```
Client                          Management Server                 Agent VM
  |                                     |                            |
  |--DetachSession------------------->|                            |
  |  {session_id: "abc123"}            |                            |
  |                                     |  (stop streaming)          |
  |<-SessionDetached------------------|                            |
  |                                     |                            |
  |  (disconnect WebSocket)            |                            |
  |                                     |  (process still running)   |
  |                                     |                            |
  |  (reconnect later)                 |                            |
  |--AttachSession------------------->|                            |
  |  {session_id: "abc123"}            |                            |
  |                                     |  (resume streaming)        |
  |<-SessionAttached------------------|                            |
  |                                     |                            |
  |<-SessionOutput--------------------|<-OutputChunk--------------|
  |  (output resumes)                  |                            |
```

#### Listing Sessions

```
Client                          Management Server                 Agent VM
  |                                     |                            |
  |--ListSessions-------------------->|                            |
  |  {agent_id: "agent-01"}            |                            |
  |                                     |  (query local state)       |
  |<-SessionList-----------------------|                            |
  |  {sessions: [                      |                            |
  |    {session_name: "dev-shell",     |                            |
  |     running: true, attached: false}|                            |
  |    {session_name: "build-watch",   |                            |
  |     running: true, attached: true} |                            |
  |  ]}                                |                            |
```

#### Killing a Session

```
Client                          Management Server                 Agent VM
  |                                     |                            |
  |--KillSession--------------------->|                            |
  |  {session_id: "abc123",            |                            |
  |   signal: 15}                      |--PtyControl-------------->|
  |                                     |  (send SIGTERM)            |
  |                                     |                            |
  |                                     |<-CommandResult------------|
  |                                     |  (exit_code: 143)          |
  |<-SessionKilled--------------------|                            |
  |  {exit_code: 143}                  |                            |
```

## REST API

### Endpoints

#### 1. Create Session

**Endpoint:** `POST /api/v1/agents/{agent_id}/sessions`

**Request Body:**
```json
{
  "session_name": "dev-shell",
  "session_type": "interactive",
  "command": "/bin/bash",
  "args": [],
  "working_dir": "/home/agent",
  "env": {
    "EDITOR": "vim"
  },
  "cols": 120,
  "rows": 40
}
```

**Response:** `201 Created`
```json
{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_name": "dev-shell",
  "session_type": "interactive",
  "agent_id": "agent-01",
  "command": "/bin/bash",
  "created_at": 1706745600000,
  "running": true,
  "attached": false
}
```

**Errors:**
- `400 Bad Request` - Invalid request (missing required fields, invalid session_type)
- `404 Not Found` - Agent not found
- `409 Conflict` - Session name already exists for this agent
- `503 Service Unavailable` - Agent not connected

#### 2. List Sessions

**Endpoint:** `GET /api/v1/agents/{agent_id}/sessions`

**Query Parameters:**
- `running` (optional): Filter by running state (`true`, `false`)
- `type` (optional): Filter by session type (`interactive`, `headless`, `background`)

**Response:** `200 OK`
```json
{
  "sessions": [
    {
      "session_id": "550e8400-e29b-41d4-a716-446655440000",
      "session_name": "dev-shell",
      "session_type": "interactive",
      "agent_id": "agent-01",
      "command": "/bin/bash",
      "args": [],
      "working_dir": "/home/agent",
      "env": {},
      "created_at": 1706745600000,
      "attached": false,
      "running": true,
      "exit_code": null,
      "pty_cols": 120,
      "pty_rows": 40
    },
    {
      "session_id": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
      "session_name": "claude-agent",
      "session_type": "headless",
      "agent_id": "agent-01",
      "command": "claude",
      "args": ["--headless", "--prompt", "Review codebase"],
      "working_dir": "/workspace",
      "env": {},
      "created_at": 1706745500000,
      "attached": false,
      "running": true,
      "exit_code": null,
      "pty_cols": null,
      "pty_rows": null
    }
  ],
  "total_count": 2
}
```

**Errors:**
- `404 Not Found` - Agent not found

#### 3. Get Session Info

**Endpoint:** `GET /api/v1/agents/{agent_id}/sessions/{session_id}`

**Response:** `200 OK`
```json
{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_name": "dev-shell",
  "session_type": "interactive",
  "agent_id": "agent-01",
  "command": "/bin/bash",
  "args": [],
  "working_dir": "/home/agent",
  "env": {},
  "created_at": 1706745600000,
  "attached": false,
  "running": true,
  "exit_code": null,
  "pty_cols": 120,
  "pty_rows": 40
}
```

**Errors:**
- `404 Not Found` - Agent or session not found

#### 4. Attach to Session

**Endpoint:** `POST /api/v1/agents/{agent_id}/sessions/{session_id}/attach`

**Request Body (optional):**
```json
{
  "cols": 120,
  "rows": 40
}
```

**Response:** `200 OK`
```json
{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_name": "dev-shell",
  "attached": true,
  "message": "Use WebSocket to receive session output"
}
```

**Errors:**
- `404 Not Found` - Agent or session not found
- `410 Gone` - Session process has exited

**Note:** This endpoint marks the session as attached for REST API tracking, but actual output streaming requires WebSocket connection with `AttachSession` message.

#### 5. Detach from Session

**Endpoint:** `POST /api/v1/agents/{agent_id}/sessions/{session_id}/detach`

**Response:** `200 OK`
```json
{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "session_name": "dev-shell",
  "attached": false,
  "running": true
}
```

**Errors:**
- `404 Not Found` - Agent or session not found

#### 6. Kill Session

**Endpoint:** `DELETE /api/v1/agents/{agent_id}/sessions/{session_id}`

**Request Body (optional):**
```json
{
  "signal": 15
}
```

**Response:** `200 OK`
```json
{
  "session_id": "550e8400-e29b-41d4-a716-446655440000",
  "killed": true,
  "exit_code": 143
}
```

**Errors:**
- `404 Not Found` - Agent or session not found
- `410 Gone` - Session already exited

**Signals:**
- `2` - SIGINT (Ctrl+C)
- `9` - SIGKILL (force kill, cannot be caught)
- `15` - SIGTERM (graceful termination, default)

## Error Handling

### Error Response Format

All error responses follow this format:

```json
{
  "error": "error_code",
  "message": "Human-readable error message",
  "details": {
    "field": "additional context"
  }
}
```

### Common Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `agent_not_found` | 404 | Agent ID does not exist |
| `agent_not_connected` | 503 | Agent exists but not connected |
| `session_not_found` | 404 | Session ID does not exist |
| `session_name_conflict` | 409 | Session name already in use for this agent |
| `session_exited` | 410 | Session process has already terminated |
| `invalid_session_type` | 400 | Session type must be interactive, headless, or background |
| `invalid_signal` | 400 | Signal number must be valid Unix signal |
| `missing_pty_dimensions` | 400 | Interactive sessions require cols and rows |
| `websocket_required` | 400 | Operation requires active WebSocket connection |

### WebSocket Error Messages

```rust
ServerMessage::Error {
    message: "Session name 'dev-shell' already exists on agent-01"
}
```

### REST Error Examples

**Session Name Conflict:**
```json
{
  "error": "session_name_conflict",
  "message": "Session name 'dev-shell' already exists on agent-01",
  "details": {
    "existing_session_id": "550e8400-e29b-41d4-a716-446655440000"
  }
}
```

**Agent Not Connected:**
```json
{
  "error": "agent_not_connected",
  "message": "Agent 'agent-01' exists but is not currently connected",
  "details": {
    "last_heartbeat": 1706745500000
  }
}
```

## Implementation Details

### Session Storage

Sessions are tracked in `CommandDispatcher` with a new `SessionManager` component:

```rust
/// Manages named sessions for agents
pub struct SessionManager {
    /// Active sessions by session_id
    sessions: RwLock<HashMap<String, Session>>,

    /// Session name index (agent_id -> session_name -> session_id)
    name_index: RwLock<HashMap<String, HashMap<String, String>>>,

    /// Attached sessions tracking (session_id -> WebSocket client IDs)
    attachments: RwLock<HashMap<String, HashSet<String>>>,
}

/// Internal session state
struct Session {
    info: SessionInfo,
    command_id: String, // Links to PendingCommand
    stdin_tx: Option<mpsc::Sender<Vec<u8>>>,
}
```

### CommandDispatcher Integration

Extend `CommandDispatcher` with session support:

```rust
impl CommandDispatcher {
    /// Create a new named session
    pub async fn create_session(
        &self,
        agent_id: &str,
        session_name: String,
        session_type: SessionType,
        command: String,
        args: Vec<String>,
        working_dir: String,
        env: HashMap<String, String>,
        cols: u32,
        rows: u32,
    ) -> Result<SessionInfo, DispatchError> {
        // Check for name conflicts
        // Create CommandRequest with session metadata
        // Store in SessionManager
        // Dispatch to agent
    }

    /// List sessions for an agent
    pub async fn list_sessions(&self, agent_id: &str) -> Vec<SessionInfo> {
        // Query SessionManager
    }

    /// Attach to session (start streaming output)
    pub async fn attach_session(
        &self,
        session_id: &str,
        client_id: &str,
    ) -> Result<SessionInfo, DispatchError> {
        // Mark session as attached
        // Return session info for client to subscribe
    }

    /// Detach from session (stop streaming output)
    pub async fn detach_session(
        &self,
        session_id: &str,
        client_id: &str,
    ) -> Result<(), DispatchError> {
        // Remove client from attachments
        // If no clients attached, mark session as detached
    }

    /// Kill session
    pub async fn kill_session(
        &self,
        session_id: &str,
        signal: i32,
    ) -> Result<Option<i32>, DispatchError> {
        // Send signal via PtyControl
        // Wait for CommandResult
        // Cleanup session
    }
}
```

### Protocol Buffer Extensions

Extend `agent.proto` to support session metadata:

```protobuf
message CommandRequest {
  // ... existing fields ...

  // Session metadata (optional)
  SessionMetadata session = 20;
}

message SessionMetadata {
  string session_id = 1;
  string session_name = 2;
  SessionType session_type = 3;
}

enum SessionType {
  SESSION_TYPE_UNKNOWN = 0;
  SESSION_TYPE_INTERACTIVE = 1;
  SESSION_TYPE_HEADLESS = 2;
  SESSION_TYPE_BACKGROUND = 3;
}
```

### Output Routing

Modify output handling to include session context:

1. `OutputMessage` includes `session_id` and `session_name` fields
2. WebSocket clients can subscribe to specific sessions
3. Detached sessions still have output captured but not streamed
4. Output buffer maintained for session reattachment

### Session Lifecycle

```
Created -> Running -> (Detached) -> Attached -> Running -> Exited
                 |                                    |
                 +------------------------------------+
                              (Detach)
```

**States:**
- `Created`: Session created, command dispatched
- `Running`: Process executing, may or may not be attached
- `Attached`: Client actively receiving output
- `Detached`: Process running, no client receiving output
- `Exited`: Process terminated (exit_code set)

### Cleanup Policy

- Sessions remain in registry for 1 hour after exit for status queries
- After 1 hour, session metadata is purged
- Running sessions are never automatically cleaned up
- Use `DELETE /sessions/{id}` to force cleanup

## Example Usage

### CLI Session Management

```bash
# Create interactive shell
curl -X POST http://localhost:8122/api/v1/agents/agent-01/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "session_name": "dev-shell",
    "session_type": "interactive",
    "command": "/bin/bash",
    "cols": 120,
    "rows": 40
  }'

# List sessions
curl http://localhost:8122/api/v1/agents/agent-01/sessions

# Get session info
curl http://localhost:8122/api/v1/agents/agent-01/sessions/{session_id}

# Kill session
curl -X DELETE http://localhost:8122/api/v1/agents/agent-01/sessions/{session_id}
```

### WebSocket Session Interaction

```javascript
// Connect to WebSocket
const ws = new WebSocket('ws://localhost:8121');

// Create session
ws.send(JSON.stringify({
  type: 'create_session',
  agent_id: 'agent-01',
  session_name: 'dev-shell',
  session_type: 'interactive',
  command: '/bin/bash',
  cols: 120,
  rows: 40
}));

// Handle session created
ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);

  if (msg.type === 'session_created') {
    console.log('Session ID:', msg.session_id);
    // Auto-attached, output will start streaming
  }

  if (msg.type === 'session_output') {
    console.log(msg.data);
  }
};

// Send input to session
ws.send(JSON.stringify({
  type: 'send_session_input',
  agent_id: 'agent-01',
  session_id: sessionId,
  data: 'ls -la\n'
}));

// Detach from session
ws.send(JSON.stringify({
  type: 'detach_session',
  agent_id: 'agent-01',
  session_id: sessionId
}));

// List sessions
ws.send(JSON.stringify({
  type: 'list_sessions',
  agent_id: 'agent-01'
}));

// Reattach later
ws.send(JSON.stringify({
  type: 'attach_session',
  agent_id: 'agent-01',
  session_id: sessionId
}));
```

### Headless Claude Agent Session

```bash
# Create headless Claude session
curl -X POST http://localhost:8122/api/v1/agents/agent-01/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "session_name": "code-review",
    "session_type": "headless",
    "command": "claude",
    "args": ["--headless", "--prompt", "Review the codebase for security issues"],
    "working_dir": "/workspace/myproject"
  }'

# Session runs in background, check status periodically
curl http://localhost:8122/api/v1/agents/agent-01/sessions/{session_id}
```

### Background Process Session

```bash
# Start long-running build watch
curl -X POST http://localhost:8122/api/v1/agents/agent-01/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "session_name": "build-watch",
    "session_type": "background",
    "command": "cargo",
    "args": ["watch", "-x", "build"],
    "working_dir": "/workspace/rust-project"
  }'

# Detach is implicit for background sessions
# Check logs via WebSocket when needed
```

## Security Considerations

1. **Session Isolation**: Sessions are isolated per agent, cannot cross agent boundaries
2. **Name Uniqueness**: Session names are unique per agent to prevent conflicts
3. **Authentication**: Session operations require same auth as command dispatch
4. **Signal Restrictions**: Only standard Unix signals allowed, validated server-side
5. **Resource Limits**: Maximum sessions per agent enforced (configurable, default: 10)
6. **Audit Logging**: All session lifecycle events logged for security audits

## Migration Path

This design is additive and backward compatible:

1. Existing `SendCommand` and `StartShell` continue to work
2. New session API provides enhanced functionality
3. Gradual UI migration from legacy commands to sessions
4. Feature flag for session API (`--enable-sessions`)

## Future Enhancements

1. **Session Recording**: Capture full session transcript for replay
2. **Session Templates**: Predefined session configurations
3. **Session Sharing**: Multiple users attach to same session (read-only observers)
4. **Session Migration**: Move session between agents (advanced use case)
5. **Session Snapshots**: Save/restore session state including scrollback buffer

## References

- `management/src/ws/connection.rs` - WebSocket message definitions
- `management/src/dispatch/dispatcher.rs` - Command dispatch logic
- `proto/agent.proto` - gRPC protocol definitions
- `management/src/http/server.rs` - REST API patterns
- `management/src/http/tasks.rs` - Task management API example

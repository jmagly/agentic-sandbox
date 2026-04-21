# Session Protocol — Domain Model & Contracts

Reference document for the formal WebSocket session protocol. Covers the domain
model, all message contracts, state machines, and key interaction sequences.
Use this to prevent drift between server (Rust) and client (JavaScript).

---

## 1. Domain Model

```mermaid
classDiagram
    class Agent {
        +String id
        +String hostname
        +String ip_address
        +AgentStatus status
        +i64 connected_at
        +i64 last_heartbeat
    }

    class Session {
        +SessionId session_id
        +String session_name
        +String command_id
        +SessionType session_type
        +String command
        +bool running
    }

    class ReplayBuffer {
        +VecDeque~SessionFrame~ frames
        +usize max_frames
        +usize max_bytes
        +usize total_bytes
        +push(frame)
        +frames_from(seq) Vec~SessionFrame~
        +oldest_seq() Option~u64~
        +newest_seq() Option~u64~
    }

    class SessionFrame {
        +SessionId session_id
        +u64 seq
        +i64 ts
        +SessionPayload payload
    }

    class SessionPayload {
        <<enumeration>>
        Output(stream, data_base64)
        Resize(cols, rows)
        RoleAssigned(role)
        ControllerChanged(controller)
        Closed(exit_code)
        Error(message)
    }

    class Attachment {
        +ClientId client_id
        +Role role
        +Sender~SessionFrame~ sender
        +usize lag
    }

    class WsConnection {
        +ClientId id
        +Map~SessionId, Role~ joined_sessions
    }

    Agent "1" --> "0..*" Session : owns
    Session "1" --> "1" ReplayBuffer : has
    Session "1" --> "0..*" Attachment : subscribers
    SessionFrame "1" --> "1" SessionPayload : contains
    ReplayBuffer "0..*" --> "0..*" SessionFrame : stores
    WsConnection "1" --> "0..*" Attachment : creates
```

---

## 2. Role Model

```mermaid
classDiagram
    class Role {
        <<enumeration>>
        Observer
        Controller
    }

    note for Role "Only one Controller per session.\nMultiple Observers allowed.\nController can send input; Observers are read-only."
```

---

## 3. WebSocket Message Contracts

### 3.1 Client → Server Messages

```mermaid
classDiagram
    class ClientMessage {
        <<enumeration>>
    }

    class StartShell {
        +String agent_id
        +Option~String~ session_name
        +u16 cols
        +u16 rows
    }

    class AttachSession {
        +String agent_id
        +String session_name
        +u16 cols
        +u16 rows
    }

    class JoinSession {
        +String session_id
        +Option~String~ role
        +Option~u64~ replay_from
    }

    class LeaveSession {
        +String session_id
    }

    class RequestControl {
        +String session_id
    }

    class YieldControl {
        +String session_id
    }

    class Input {
        +String command_id
        +String data
    }

    class Resize {
        +String command_id
        +u16 cols
        +u16 rows
    }

    class ListSessions {
        +String agent_id
    }

    class KillSession {
        +String session_id
        +String agent_id
    }

    ClientMessage <|-- StartShell
    ClientMessage <|-- AttachSession
    ClientMessage <|-- JoinSession
    ClientMessage <|-- LeaveSession
    ClientMessage <|-- RequestControl
    ClientMessage <|-- YieldControl
    ClientMessage <|-- Input
    ClientMessage <|-- Resize
    ClientMessage <|-- ListSessions
    ClientMessage <|-- KillSession
```

### 3.2 Server → Client Messages

```mermaid
classDiagram
    class ServerMessage {
        <<enumeration>>
    }

    class AgentList {
        +Vec~AgentInfoWs~ agents
    }

    class ShellStarted {
        +String agent_id
        +String command_id
        +String session_name
    }

    class Output {
        +String agent_id
        +String command_id
        +String data
    }

    class SessionList {
        +String agent_id
        +Vec~SessionInfoWs~ sessions
    }

    class SessionAttached {
        +String agent_id
        +String command_id
        +String session_name
        +String session_id
    }

    class SessionJoined {
        +String session_id
        +String role
        +u64 current_seq
    }

    class SessionLeft {
        +String session_id
    }

    class SessionFrame {
        +String session_id
        +u64 seq
        +i64 ts
        +SessionPayload payload
    }

    class ControlGranted {
        +String session_id
    }

    class ControlDenied {
        +String session_id
        +String reason
    }

    class Error {
        +String message
    }

    ServerMessage <|-- AgentList
    ServerMessage <|-- ShellStarted
    ServerMessage <|-- Output
    ServerMessage <|-- SessionList
    ServerMessage <|-- SessionAttached
    ServerMessage <|-- SessionJoined
    ServerMessage <|-- SessionLeft
    ServerMessage <|-- SessionFrame
    ServerMessage <|-- ControlGranted
    ServerMessage <|-- ControlDenied
    ServerMessage <|-- Error
```

### 3.3 SessionFrame Payload Wire Format

`SessionFrame` is tagged with `kind` (snake_case). All payload fields are
flattened into the top-level JSON object.

```mermaid
classDiagram
    class SessionFrameWire {
        +String type = "session_frame"
        +String session_id
        +u64 seq
        +i64 ts
        +String kind
    }

    class OutputPayload {
        +String kind = "output"
        +String stream
        +String data
    }

    class ResizePayload {
        +String kind = "resize"
        +u16 cols
        +u16 rows
    }

    class RoleAssignedPayload {
        +String kind = "role_assigned"
        +String role
    }

    class ClosedPayload {
        +String kind = "closed"
        +Option~i32~ exit_code
    }

    class ErrorPayload {
        +String kind = "error"
        +String message
    }

    SessionFrameWire <|-- OutputPayload : kind discriminant
    SessionFrameWire <|-- ResizePayload : kind discriminant
    SessionFrameWire <|-- RoleAssignedPayload : kind discriminant
    SessionFrameWire <|-- ClosedPayload : kind discriminant
    SessionFrameWire <|-- ErrorPayload : kind discriminant
```

**Important**: `data` in `OutputPayload` is **base64-encoded raw PTY bytes**.
Clients must `atob(data)` and write as `Uint8Array` to xterm, not as a string.

`stream` values: `"stdout"` | `"stderr"` | `"log"`

---

## 4. Session State Machine

```mermaid
stateDiagram-v2
    [*] --> Pending : StartShell sent
    Pending --> Running : ShellStarted received
    Running --> Running : Output / Resize / Input
    Running --> Joined : JoinSession sent (observer)
    Running --> Joined : JoinSession sent (controller)
    Joined --> Joined : session_frame streaming
    Joined --> Running : LeaveSession sent
    Joined --> Closed : session_frame(kind=closed)
    Running --> Closed : session killed / process exit
    Closed --> [*]

    note right of Joined
        Server replays ring buffer
        from replay_from seq,
        then streams live frames.
    end note
```

---

## 5. Interaction Sequences

### 5.1 New Shell Session (first connect)

```mermaid
sequenceDiagram
    participant UI as Browser (app.js)
    participant WS as WebSocket (connection.rs)
    participant Disp as Dispatcher
    participant PTY as PTY Process

    UI->>WS: list_sessions { agent_id }
    WS-->>UI: session_list { sessions: [] }
    UI->>WS: start_shell { agent_id, cols, rows }
    WS->>Disp: spawn PTY
    Disp->>PTY: fork + exec shell
    WS-->>UI: shell_started { agent_id, command_id, session_name }
    loop Output streaming
        PTY-->>Disp: PTY bytes
        Disp->>WS: publish_output → broadcast
        WS-->>UI: output { command_id, data }
    end
```

### 5.2 Reconnect with Replay (JoinSession)

```mermaid
sequenceDiagram
    participant UI as Browser (app.js)
    participant WS as WebSocket (connection.rs)
    participant Reg as SessionRegistry
    participant Buf as ReplayBuffer

    Note over UI: Page load / hard refresh
    UI->>WS: list_sessions { agent_id }
    WS-->>UI: session_list { sessions: [{ session_id, session_name, ... }] }
    Note over UI: interactive session found →<br/>attachExistingSession()
    UI->>WS: join_session { session_id, role: "observer", replay_from: 0 }
    WS->>Reg: attach(session_id, client_id, replay_from=0)
    Reg->>Buf: frames_from(0)
    Buf-->>Reg: buffered frames [seq=0..N]
    loop Replay buffered frames
        Reg-->>WS: SessionFrame (seq 0..N)
        WS-->>UI: session_frame { kind, seq, data, ... }
        UI->>UI: handleSessionFrame → atob(data) → term.write(bytes)
    end
    WS-->>UI: session_joined { session_id, role, current_seq }
    loop Live streaming
        WS-->>UI: session_frame { kind: "output", seq: N+1, ... }
        UI->>UI: update lastSeqPerSession[session_id]
    end
```

### 5.3 Incremental Reconnect (after disconnect)

```mermaid
sequenceDiagram
    participant UI as Browser (app.js)
    participant WS as WebSocket (connection.rs)
    participant Buf as ReplayBuffer

    Note over UI: WS closes unexpectedly
    UI->>UI: onClose → reconnect backoff
    UI->>WS: (reconnect)
    UI->>WS: list_sessions { agent_id }
    WS-->>UI: session_list { sessions: [...] }
    Note over UI: replay_from = lastSeqPerSession[session_id] + 1
    UI->>WS: join_session { session_id, replay_from: last_seq+1 }
    WS->>Buf: frames_from(last_seq+1)
    Buf-->>WS: only missed frames
    loop Missed frames only
        WS-->>UI: session_frame { seq: last_seq+1 .. N }
    end
    WS-->>UI: session_joined { current_seq: N }
```

### 5.4 Controller Role Escalation

```mermaid
sequenceDiagram
    participant UI as Browser (app.js)
    participant WS as WebSocket (connection.rs)
    participant Reg as SessionRegistry

    Note over UI: User wants to type (needs controller)
    UI->>WS: request_control { session_id }
    WS->>Reg: try_grant_controller(session_id, client_id)
    alt Controller slot free
        Reg-->>WS: granted
        WS-->>UI: control_granted { session_id }
        WS-->>UI: session_frame { kind: "role_assigned", role: "controller" }
        UI->>WS: input { command_id, data: "<keystrokes>" }
    else Controller already held
        Reg-->>WS: denied
        WS-->>UI: control_denied { session_id, reason: "held by <client>" }
    end
```

### 5.5 Suicide Snail Eviction (planned — Issue #149)

```mermaid
sequenceDiagram
    participant PTY as PTY Process
    participant Reg as SessionRegistry
    participant SlowWS as Slow WS Client
    participant FastWS as Fast WS Client

    loop High-frequency output
        PTY-->>Reg: publish_output(frame)
        Reg->>FastWS: try_send(frame) → OK, lag reset to 0
        Reg->>SlowWS: try_send(frame) → Err (full), lag++
    end
    Note over Reg: lag >= 500
    Reg->>SlowWS: drop Sender (closes channel)
    SlowWS-->>SlowWS: Receiver closed → WS close frame sent
    Note over SlowWS: Client reconnects, replays missed frames via JoinSession
```

---

## 6. SessionId vs CommandId

These are frequently confused. This table is authoritative:

| Field | Type | Scope | Used for |
|---|---|---|---|
| `session_id` | `String` (UUID) | Stable across reconnects | `join_session`, `leave_session`, `request_control`, `session_frame` routing |
| `command_id` | `String` (UUID) | PTY process lifetime | `output` message routing, `input`, `resize`, legacy `attach_session` |

- `session_id` is created when a session is registered in `SessionRegistry`.
- `command_id` is created when the PTY process is spawned by the Dispatcher.
- One session has exactly one `command_id` for its lifetime.
- Clients using the formal protocol (JoinSession) use only `session_id`.
- Clients using the legacy protocol (attach_session) use `command_id` from `session_attached`.

---

## 7. Data Encoding

| Field | Encoding | Reason |
|---|---|---|
| `output.data` in `session_frame` | Base64 (standard) | PTY output is binary — ESC sequences, null bytes |
| `output.data` in legacy `output` message | UTF-8 string | Legacy path, may corrupt binary sequences |
| `ts` | Unix millis (i64) | Monotonic wall time for replay ordering |
| `seq` | u64, monotonically increasing per session | Replay cursor — safe to persist in localStorage |

**Client decode pattern** (always use this for `session_frame` output):
```js
const raw = atob(msg.data);
const bytes = new Uint8Array(raw.length);
for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
term.write(bytes);
```

---

## 8. Client-Side State Map

```mermaid
classDiagram
    class AgenticDashboard {
        +Map~agentId, AgentInfo~ agents
        +Map~agentId, PaneEntry~ panes
        +Map~agentId, String~ activeCommandIds
        +Map~agentId, String~ shellCommandIds
        +Set~agentId~ pendingStartupAttach
        +Map~sessionId, agentId~ sessionIdToAgentId
        +Map~sessionId, u64~ lastSeqPerSession
        +Map~commandId, Buffer~ sessionBuffers
    }

    note for AgenticDashboard "sessionIdToAgentId: routes session_frame to correct pane\nlastSeqPerSession: persisted replay cursor (localStorage pending #144)\nactiveCommandIds: legacy output routing by command_id"
```

---

## 9. Open Implementation Gaps

| Issue | Title | Status |
|---|---|---|
| #143 | Use JoinSession for history-preserving reconnect | **Closed** (fa6ee4a) |
| #144 | Persist last_seq in localStorage | Open |
| #145 | Periodic keyframe injection | Open |
| #146 | broadcast() holds Mutex during fan-out | Open |
| #147 | ReplayBuffer stores base64 (25% overhead) | Open |
| #148 | Ring buffer not byte-capped | Open |
| #149 | No suicide snail for slow WS clients | Open |

---

## 10. File Map

| Concern | File |
|---|---|
| WS message types (Rust) | `management/src/ws/connection.rs` — `ClientMessage`, `ServerMessage` |
| Session domain (Rust) | `management/src/session/mod.rs` — `Session`, `SessionFrame`, `SessionPayload` |
| Ring buffer (Rust) | `management/src/session/replay.rs` — `ReplayBuffer` |
| Session registry (Rust) | `management/src/session/registry.rs` — `SessionRegistry`, `Attachment` |
| Client protocol (JS) | `management/ui/app.js` — `handleMessage`, `handleSessionFrame`, `attachExistingSession` |

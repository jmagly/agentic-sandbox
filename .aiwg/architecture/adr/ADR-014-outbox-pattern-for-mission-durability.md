# ADR-014: Outbox Pattern for Mission Durability

## Status

**Reframed under ADR-018 (A2A as base protocol).** Outbox semantics unchanged; what's stored changes. The outbox now persists **A2A `Task` state + `Artifact` records + push-notification configs + idempotency cache entries** rather than CloudEvents-shaped wire envelopes. Resume flow uses A2A's `SubscribeToTask` (initial Task state + future stream from now) instead of `?since=<seq>` event cursor — the outbox guarantees the Task state is current when the orchestrator re-subscribes. SQLite schema below stays mostly the same; column names shift from `mission_*` to `task_*`. See `.aiwg/working/issue-planner/a2a-gap-matrix.md` rows 5, 19, 20.

Original disposition: Proposed

## Date

2026-05-09

## Context

v1 persists mission state to `<secrets_dir>/../missions.json` via atomic `tmp + rename`. Events are pushed directly to the orchestrator over WebSocket; no durable record of *what events were emitted*. If the executor crashes between (a) updating mission state and (b) pushing the event, the orchestrator never sees the event. Restart recovery (`executor.resync`) is coarse — it re-asserts the active mission set but cannot replay missed events.

Research (§A.3 outbox, §C-31 Temporal history replay) shows the **transactional outbox pattern** as the standard solution:

1. Mission state change and "event to publish" are written to a local table in **the same transaction**.
2. A separate publisher loop reads pending events, publishes to the orchestrator, and marks them shipped.
3. Crash during publish → on restart, publisher re-publishes (consumer dedupes by event `id`).

This gives **exactly-once-effective delivery to a deduping consumer** without requiring distributed transactions.

## Decision

**Formalize `missions.json` into a durable outbox table that holds both mission state and pending outbound events. Refactor the publish path to read from outbox.**

### Storage

- **Backend**: SQLite via `rusqlite` (familiar to project; embedded; ACID; small footprint). Considered sled (kv) and rocksdb; SQLite wins for the relational shape (mission ↔ events) and operator inspectability (`sqlite3 missions.db`).
- **Location**: `<secrets_dir>/../missions.db` (replaces `missions.json`).
- **Schema**:

```sql
CREATE TABLE missions (
  mission_id TEXT PRIMARY KEY,
  state TEXT NOT NULL,           -- queued | assigned | running | hitl_required | suspended | paused | completed | failed | errored | aborted
  fail_kind TEXT,                -- application | infrastructure (when state=failed/errored)
  manifest_json TEXT NOT NULL,
  metadata_json TEXT,
  next_seq INTEGER NOT NULL DEFAULT 0,  -- monotonic per-mission event sequence
  created_at TEXT NOT NULL,      -- RFC 3339
  updated_at TEXT NOT NULL,
  terminal_at TEXT               -- set when entering terminal state
);

CREATE TABLE mission_events (
  event_id TEXT PRIMARY KEY,     -- CloudEvents id
  mission_id TEXT NOT NULL REFERENCES missions(mission_id),
  seq INTEGER NOT NULL,          -- per-mission monotonic; (mission_id, seq) UNIQUE
  envelope_json TEXT NOT NULL,   -- full CloudEvents envelope
  shipped INTEGER NOT NULL DEFAULT 0,  -- 0=pending, 1=delivered to orchestrator at least once
  shipped_at TEXT,
  retention_until TEXT NOT NULL  -- 24h or last-1000-events boundary, whichever is later
);

CREATE INDEX mission_events_pending_idx ON mission_events(mission_id, seq) WHERE shipped = 0;
CREATE INDEX mission_events_retention_idx ON mission_events(retention_until);

CREATE TABLE idempotency_cache (
  key TEXT PRIMARY KEY,
  request_hash TEXT NOT NULL,
  response_status INTEGER NOT NULL,
  response_body TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL       -- created_at + 24h
);

CREATE INDEX idempotency_expiry_idx ON idempotency_cache(expires_at);
```

### Write path

```
Mission state change (e.g., dispatch handler):
  BEGIN TRANSACTION
    UPDATE missions SET state = 'running', next_seq = next_seq + 1 WHERE mission_id = ?
    INSERT INTO mission_events(event_id, mission_id, seq, envelope_json, retention_until)
      VALUES (?, ?, ?, ?, datetime('now', '+24 hours'))
  COMMIT
  // publisher loop will pick up the new event
```

### Publish path

```
Publisher loop (one task per executor connection):
  SELECT event_id, envelope_json FROM mission_events
    WHERE shipped = 0 AND mission_id IN (SELECT mission_id FROM missions WHERE ... )
    ORDER BY seq
    LIMIT 100;
  For each event:
    Send over WS
    On ack (or timeout fallback):
      UPDATE mission_events SET shipped = 1, shipped_at = datetime('now') WHERE event_id = ?
  Sleep 50ms; repeat.
```

WS doesn't have application-level acks by default — the spec adds `mission.ack` (orchestrator → sandbox: "I've processed seq=N") to enable reliable shipped-marking. Without acks, "shipped=1 after socket write" is a weaker guarantee but acceptable for at-least-once.

### Resume path (`?since=<seq>` cursor)

```
WS connect with ?since=<seq>&mission=<id>:
  SELECT envelope_json FROM mission_events
    WHERE mission_id = ? AND seq > ? AND retention_until > datetime('now')
    ORDER BY seq;
  Replay each event over the new connection.
  Then continue with new events from publisher loop.
```

### Retention / GC

Background task every hour:

```
DELETE FROM mission_events WHERE retention_until < datetime('now')
  AND mission_id IN (SELECT mission_id FROM missions WHERE state IN ('completed','failed','errored','aborted')
                      AND terminal_at < datetime('now','-7 days'));

DELETE FROM idempotency_cache WHERE expires_at < datetime('now');

DELETE FROM missions WHERE state IN ('completed','failed','errored','aborted')
  AND terminal_at < datetime('now','-30 days');
```

Mission record retained 30 days after terminal (operator forensics); events retained 24h or last 1000 per mission.

## Alternatives Considered

| Option | Pros | Cons |
|---|---|---|
| **A. SQLite outbox (chosen)** | ACID; embedded; operator-inspectable; familiar | Adds rusqlite dependency; ~1MB SO file |
| B. sled (kv store, embedded) | Pure Rust; no SQL | Less inspectable; relational queries awkward |
| C. RocksDB | High-performance kv | Heavyweight (~20MB); overkill for our QPS |
| D. JSON file with WAL (extend v1) | No new dep | Reinventing SQLite badly; race-prone |
| E. External Postgres | Production-grade | Operational complexity; runs counter to single-binary deployment |
| F. Redis | Fast | Doesn't survive crash without AOF; new deployment dependency |

## Consequences

### Positive

- True at-least-once delivery: events durable before publish attempt.
- `?since=<seq>` resume cursor implementable correctly.
- Idempotency cache (ADR-008) shares the same store with same crash semantics.
- Operator can `sqlite3 missions.db 'SELECT state, COUNT(*) FROM missions GROUP BY state'` for diagnostics.
- Restart-recovery test in conformance harness is straightforward.

### Negative

- New dependency: `rusqlite` (well-maintained, common in Rust ecosystem; low risk).
- Migration from v1's `missions.json`: one-time tool reads JSON, populates DB, archives JSON. Tested in agentic-sandbox CI.
- Risk R-6: outbox introduces new failure modes (DB lock, disk full mid-write). Mitigation: explicit error handling, fall back to ephemeral queue with operator alert if DB unwritable.
- Slight write-path latency increase (vs. JSON write): SQLite single-row insert is ~0.5ms on tmpfs, ~3ms on SSD. Negligible vs. mission durations.

### Neutral

- v1 path keeps `missions.json` until v2.0; v2.0 migrates on first start.

## Implementation Notes

- Use `rusqlite` with `bundled` feature for self-contained binary.
- Connection pooling: `r2d2_sqlite` or single connection with mutex (start with single — our QPS is low).
- WAL mode for crash safety: `PRAGMA journal_mode=WAL;`.
- Backup: SQLite's online backup API for hot copies.
- Conformance test: kill the executor between mission state update and event publish; verify on restart that the event is published exactly once (orchestrator dedupes by event `id`).

## Related

- Synthesis C12
- Best-practices research §3 (outbox), §C-31 (Temporal history replay)
- ADR-008 (idempotency cache shares storage)
- Vision §4 success criterion S5
- Risk R-6 (outbox failure modes)
- v1 baseline: `management/src/aiwg_serve.rs` `MissionStore` impl

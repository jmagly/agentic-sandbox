# UC-008: Executor Restart and Resync

## ID

UC-008

## Primary Actor

agentic-sandbox executor process (after crash, restart, or planned redeploy)

## Stakeholders

- **Sandbox operator**: needs the system to recover gracefully without manual intervention.
- **Orchestrator**: needs to know which missions are still active vs. lost.
- **End user**: needs in-flight missions to complete, not silently disappear.

## Goal

When the agentic-sandbox executor process restarts (planned or unplanned), in-flight missions and their event histories survive; orchestrator reconnects, learns which missions are still active, and resumes event consumption from the correct cursor with no missed events.

## Pre-conditions

- Sandbox was running with one or more active missions in non-terminal states (`queued`, `assigned`, `running`, `hitl_required`, `paused`, `suspended`).
- Mission state and events persist in outbox (`<secrets_dir>/../missions.db`).
- Orchestrator was connected via WS and has its `last_seen_seq` per mission.

## Main Flow

1. Executor process exits (crash, signal, planned restart).
2. WS connections drop on the orchestrator side; orchestrator enters reconnect-with-backoff loop.
3. Sandbox process restarts:
   - Reads outbox DB to recover mission records.
   - Computes active mission set (non-terminal states).
   - Marks missions in `running` state as `suspended` if their agent process is no longer alive (sandbox VM/container check).
   - Re-establishes WS server endpoints.
4. Orchestrator reconnects to `ws://host:8121/ws/executors/{executor_id}` with `?since=<last_seen_seq>` per active mission (typically reconnects to the channel and replays per-mission).
5. Sandbox sends `server_hello` first frame, awaits `client_hello` (capability re-negotiation).
6. Sandbox sends `executor.resync` event listing active mission IDs:
   ```json
   {
     "type": "io.aiwg.executor.executor.resync",
     "id": "<event uuid>",
     "data": {
       "active_missions": [
         {"mission_id": "...", "state": "running", "last_event_seq": 42},
         {"mission_id": "...", "state": "hitl_required", "last_event_seq": 17}
       ],
       "executor_uptime_s": 3
     }
   }
   ```
7. For each active mission, orchestrator can:
   - Accept (continue consuming events) — sandbox resumes publishing from `last_event_seq + 1` based on outbox.
   - Abort (orchestrator no longer cares) — orchestrator POSTs `/api/v2/missions/{id}/abort` per UC-006 A4 logic.
8. Sandbox resumes per-mission event publishing from outbox; orchestrator deduplicates by event `id`.
9. Missions in `suspended` state can be resumed by sandbox if the agent process is restartable (containers: trivially; VMs: via re-attach to running PTY).
10. If agent process is unrecoverable, sandbox emits `mission.errored` with `fail_kind: infrastructure` and reason "executor restart could not recover agent process".

## Alternative Flows

### A1. Orchestrator never reconnects

- Sandbox publisher loop has no consumer; events accumulate in outbox.
- After configurable timeout (default 5 minutes of no orchestrator presence), sandbox emits a operator-visible warning via `/metrics` and continues (events still durable).
- When orchestrator eventually reconnects, normal resume flow applies.

### A2. Outbox storage corrupted on restart

- Sandbox detects DB corruption (SQLite integrity check fails on startup).
- Sandbox refuses to start with that DB; logs error; alerts operator.
- Operator restores from backup or accepts data loss; sandbox starts with clean DB.
- Orchestrator reconnects, sees `executor.resync` with `active_missions: []` → treats all previously-known missions as lost.

### A3. Mission record exists but agent process is gone (container/VM was destroyed)

- Sandbox marks state `suspended` on restart.
- If `mission.runtime` is restartable (container with persistent volume): sandbox restarts container, emits `mission.reconnected` then resumes execution.
- If not restartable (VM was destroyed): sandbox emits `mission.errored` (`fail_kind: infrastructure`); mission is lost cleanly.

### A4. Orchestrator reconnects with stale `last_seen_seq` (older than retention window)

- Sandbox detects requested `since=<seq>` is older than oldest retained event.
- Returns WS close code 1011 + reason `RESUME_CURSOR_EXPIRED`.
- Orchestrator treats mission as needing fresh state — typically asks via `/api/v2/missions/{id}` for current state and decides whether to abort or accept partial visibility.

### A5. Multiple sandboxes (HA scenario, future)

- Out of v2.0 scope. Cross-host event-log replay deferred to v3+ (Vision §5).

## Post-conditions

- All durable events delivered at least once (outbox guarantees this).
- Orchestrator's view of active missions is consistent with sandbox's outbox.
- Lost data is bounded by the retention window (24h or 1000 events per mission, whichever is larger).
- Operator can inspect outbox DB for forensics: `sqlite3 missions.db 'SELECT mission_id, state, terminal_at FROM missions ORDER BY updated_at DESC LIMIT 20'`.

## Acceptance Criteria

- AC-1: Executor restart with N active missions: all N survive in outbox; non-terminal states preserved.
- AC-2: Orchestrator reconnect with `?since=<last_seen_seq>` replays only events since that seq, in order, with no duplicates beyond consumer-side dedup.
- AC-3: `executor.resync` lists exactly the active mission set.
- AC-4: Conformance harness can `kill -9` the executor mid-mission, restart, and verify mission survives + events deliver.
- AC-5: Restart RPO is 0: no mission state change is lost across restart boundary (outbox writes are atomic).
- AC-6: Restart MTTR target: executor process re-establishes WS endpoints within 5s of process start.
- AC-7: Operator alert fires if outbox storage is full or corrupted on startup.

## Related

- ADR-014 (outbox)
- ADR-009 (capability re-negotiation on reconnect)
- ADR-010 (conformance harness verifies AC-4)
- v1 baseline: `executor.resync` exists in v1; v2 makes it event-driven and cursor-resumable
- Vision §4 success criterion S5

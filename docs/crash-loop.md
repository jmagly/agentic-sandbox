# Crash Loop Detection

The crash loop detector watches VM lifecycle events for the failure
pattern "boots, crashes, boots again, crashes again" and (when
enabled) triggers an automatic rebuild via `provision-vm.sh`. It is
the management server's auto-remediation surface for VMs that fail
to come up cleanly after a configuration change, kernel update, or
provisioning regression.

Source: [`management/src/crash_loop.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/crash_loop.rs).
Lifecycle event source: [`management/src/libvirt_events.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/libvirt_events.rs).

This document also covers the lighter mission-level poison-pill detector used
by the AIWG executor integration. Container instances are swept by
`docker_runtime`'s orphan loop ([`container-runtime.md`](container-runtime.md))
without auto-rebuild — operators decide whether to respawn.

---

## Detection semantics

A "crash" is a VM lifecycle event of type `VmEventType::Crashed`, or
a `VmEventType::Stopped` whose `reason` field is set to `"crashed"`.
Clean shutdowns (`Shutdown`, `Stopped` without a crash reason) do
not count.

The detector keeps a `VmCrashHistory` per VM in
`Arc<RwLock<HashMap<String, VmCrashHistory>>>`. Each history tracks:

- `state: VmState` — one of `Healthy`, `Starting`, `Recovering`,
  `CrashLoop`, `Rebuilding`, `Failed`.
- `restart_events: Vec<CrashEvent>` — rolling list of crash events
  with timestamps and uptime-at-crash.
- `rebuild_count: u32` — total automatic rebuilds attempted.
- `last_healthy_boot: Option<DateTime<Utc>>` — timestamp of the
  last boot that exceeded `min_uptime_seconds`.
- `last_rebuild: Option<DateTime<Utc>>` — used for cooldown checks.

A VM enters `CrashLoop` state when its rolling-window crash count
exceeds `max_restarts` (default 5) within `window_minutes` (default
10). Boots are only counted as "healthy" — and thus eligible to
reset the counter — if the VM stayed up for at least
`min_uptime_seconds` (default 60).

AIWG missions use a separate `MissionCrashLoopStatus` stored on each
`MissionRecord`. It bounds reconnect/resume loops rather than VM process
crashes. A suspended, assigned, or HITL-paused mission increments its mission
counter when executor resync attempts to resume it. Already-running missions do
not increment the counter on ordinary WebSocket reconnects. When the threshold
is reached the mission state becomes `quarantined`, which is terminal for
resync purposes and preserves the mission for operator review instead of
replaying it.

---

## Configuration

`CrashLoopConfig` ([`crash_loop.rs:22`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/crash_loop.rs)):

| Field | Default | Purpose |
|---|---|---|
| `max_restarts` | `5` | Crashes in window before declaring crash loop. |
| `window_minutes` | `10` | Rolling window for counting crashes. |
| `min_uptime_seconds` | `60` | Minimum uptime to count a boot as "healthy". |
| `healthy_reset_minutes` | `5` | Continuous healthy time that resets the restart counter. |
| `remediation_enabled` | `true` | Master switch for auto-rebuild. Set false to detect only. |
| `max_rebuild_attempts` | `3` | Rebuild ceiling; VM goes to `Failed` after this many. |
| `rebuild_cooldown_minutes` | `30` | Minimum gap between rebuilds for the same VM. |
| `provision_script` | `images/qemu/provision-vm.sh` | Script invoked on rebuild. |
| `data_dir` | `/var/lib/agentic-sandbox/vms` | Crash history persistence. |

These are constructed in code today; an env-var loader can be added
without changing the data shape. The defaults are calibrated for
the `agentic-dev` profile — workloads with longer legitimate boot
times (large initial disk layout, expensive cloud-init) should
raise `min_uptime_seconds`.

`MissionCrashLoopConfig`
([`aiwg_serve/mod.rs`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/aiwg_serve/mod.rs)):

| Field | Default | Purpose |
|---|---|---|
| `max_consecutive_failures` | `3` | Resume/reconnect attempts in window before quarantining a mission. |
| `window_minutes` | `10` | Rolling window for the mission failure counter. |

The mission detector is intentionally conservative: it quarantines the mission
record and emits `mission.failed` with `state: "failed_preserved"`; it does not
delete sessions, kill VMs, or retry on its own.

---

## State machine

```
                        ┌─────────┐
                  ┌────▶│ Healthy │◀───────────┐
                  │     └────┬────┘            │
                  │          │ crash           │
       healthy    │          ▼                 │
       window     │     ┌──────────┐  crash    │
       elapsed    │     │ Starting │───────┐   │
                  │     └────┬─────┘       │   │
                  │          │ uptime>min  │   │
                  │          └─────────────┘   │
                  │                            │
                  │     ┌───────────┐          │
                  ├─────│Recovering │◀─────────┤
                  │     └────┬──────┘          │
                  │          │ window exceeded │
                  │          ▼                 │
                  │     ┌───────────┐          │
                  │     │CrashLoop  │──────────┤ rebuild OK
                  │     └────┬──────┘          │
                  │          │ rebuild         │
                  │          ▼                 │
                  │     ┌──────────┐           │
                  │     │Rebuilding│───────────┘
                  │     └────┬─────┘
                  │          │ max attempts
                  │          ▼
                  │     ┌─────────┐
                  └──── │ Failed  │ (manual operator unblock)
                        └─────────┘
```

`Failed` is terminal until the operator intervenes. The detector
will not retry automatically — too many rebuild attempts is a
strong signal the rebuild itself is broken, not a transient fault.

Mission `quarantined` is also terminal for executor resync. Quarantined
missions are omitted from future `executor.resync` ownership lists, so the loop
stops after the threshold. The persisted mission record keeps
`crash_loop.consecutive_failures`, `window_started_at`,
`last_failure_reason`, and `quarantined_at` for postmortem review.

---

## Notifications

`CrashLoopDetector::with_notifications(tx)` accepts an
`mpsc::Sender<CrashLoopNotification>`. The struct
([`crash_loop.rs:160`](https://github.com/jmagly/agentic-sandbox/blob/main/management/src/crash_loop.rs))
carries:

```rust
pub struct CrashLoopNotification {
    pub vm_name: String,
    pub event_type: String,        // "crash_loop_detected" / "rebuild_started" / …
    pub state: VmState,
    pub restart_count: u32,
    pub rebuild_count: u32,
    pub timestamp: DateTime<Utc>,
    pub message: String,
}
```

Operator-visible side effects:

- **Event store.** Each notification flows through the same
  `events::EventStore` as VM lifecycle events; visible on the
  dashboard's Events panel and the `/api/v1/events?follow=true`
  SSE stream documented in [`transport-audit.md`](transport-audit.md).
- **Metrics.** State transitions are recorded by the `Metrics`
  aggregator (see [`telemetry.md`](telemetry.md)). The relevant
  series surface VM restart counts and current state by VM name.
- **Tracing.** `info!`, `warn!`, and `error!` events flow into
  the in-memory ring buffer ([`transport-audit.md`](transport-audit.md))
  so operators can correlate the detector's view with the rest of
  the management server log.
- **AIWG status API.** `/api/v1/aiwg/status` includes
  `mission_crash_loop.config`, `mission_crash_loop.quarantined_count`, and the
  mission records with their `crash_loop` status.
- **Dashboard.** The AIWG status badge tooltip shows the quarantined mission
  count and up to three quarantined mission IDs with failure count and last
  recorded reason.

---

## Recovery actions

When a VM enters `CrashLoop` state and `remediation_enabled` is
true, the detector invokes the `provision_script` (default
`provision-vm.sh`) to rebuild the VM in place. The script:

1. `virsh destroy` the running domain (graceful if possible).
2. Wipes the disk image.
3. Re-runs cloud-init from the recorded loadout.
4. Boots the VM.

`max_rebuild_attempts` (default 3) caps how many times this fires.
After the cap, the VM is parked in `Failed` and the detector emits
a final notification with `event_type: "max_rebuilds_exceeded"`.
Operator response is to either:

- Inspect what's wrong (`virsh console`, journal from the most
  recent boot, the cloud-init log captured to agentshare).
- Manually reset the VM's history (`rm -rf
  /var/lib/agentic-sandbox/vms/<name>`) and `provision-vm.sh
  --destroy <name> && provision-vm.sh <name>` from a clean slate.
- Disable auto-remediation for this VM and treat the host as
  pinned for forensic analysis.

If the operator just wants to give the detector another chance —
say after a bad disk image was replaced — restarting the management
server is enough; the in-memory state resets and the next crash
event starts the counter from zero. Persisted history under
`data_dir` keeps the audit trail.

---

## Operational guidance

- **Always validate VMs post-provision.** A VM that comes up
  briefly and then crashes inside `min_uptime_seconds` will be
  counted as a crash, not a healthy boot — the detector cannot
  tell "boot worked but agent never started" from "boot failed
  early". Pair the detector with `validate-vm.sh` style checks
  before declaring a VM healthy.
- **Don't tune `min_uptime_seconds` to mask flakiness.** Raising
  it from 60 s to 600 s "to stop the alerts" hides the underlying
  problem. If a VM legitimately takes 10 minutes to come up, raise
  it; if it's crashing 4 minutes in, fix the crash.
- **Cooldown is not jitter.** The 30-minute default
  `rebuild_cooldown_minutes` is there so an operator can land a
  config fix between rebuilds. It is **not** there to space out
  rebuilds that all hit the same broken artifact — that's what
  `max_rebuild_attempts` is for.
- **The `Failed` state is intentional.** A VM that has burned
  through 3 auto-rebuilds is a debugging target, not a workload
  to keep retrying. Don't add a "retry forever" knob.

---

## See also

- [`vm-lifecycle.md`](vm-lifecycle.md) — full VM state machine
  (the detector overlays auto-remediation on top of this).
- [`telemetry.md`](telemetry.md) — metrics labels for VM state
  and rebuild counts.
- [`transport-audit.md`](transport-audit.md) — where crash-loop
  notifications surface in `/api/v1/logs` and `/api/v1/events`.
- [`container-runtime.md`](container-runtime.md) — why the
  container side is operator-driven instead.
- `images/qemu/provision-vm.sh` — the script the detector invokes
  for rebuilds.

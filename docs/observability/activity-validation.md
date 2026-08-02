# Bounded activity canaries and failure drills

`scripts/observability/activity_validation.py` adds a small synthetic layer to
the passive rolling evidence ledger. It is designed for timer invocation during
normal operation; it does not need an idling agent or a continuous load runner.
Every emitted sample is immutable `canary` or `drill` evidence, so none can
satisfy organic one-hour, 24-hour, or seven-day duration.

## Known-signal canary

The canary writes one metadata-only `validation.canary` event through the fixed
activity ingest endpoint, then requires the exact UUIDv7 event to appear in both
query and signed export. Its ledger record includes the unique identity,
collector sequence, expected deadline, per-phase latency, visibility, and
failure. Persistent state enforces the configured minimum interval and maximum
outstanding count. The reference budget is one event per 15 minutes, at most two
outstanding, with a two-minute deadline.

Dry-run resolves the event identity, exact endpoints, sequence, deadline, and
budgets without sending a request or writing state:

```bash
python3 scripts/observability/activity_validation.py \
  --config configs/activity-validation.json \
  --evidence-config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  canary --dry-run
```

Remove `--dry-run` to execute. If the management boundary requires a bearer
credential, set `operator_bearer_token_file` to a root/operator-owned file with
no group or world permissions. The value is read only into the request header;
it is never placed in config output, state, or evidence. Prefer the existing
local authenticated operator boundary where available.

## Fixed drill profiles

The checked configuration contains exactly five executors:

| Profile | Hypothesis exercised | Default scheduling |
| --- | --- | --- |
| `fixture_collector_restart` | restart and recovery evidence is continuous | daily disposable fixture |
| `fixture_exporter_outage` | a brief outage is visible and buffered state recovers | timer-eligible, not scheduled by default |
| `fixture_backpressure` | queue pressure stays below its cap and clears | timer-eligible, not scheduled by default |
| `fixture_authorization_rejection` | denial is visible without protected-state change | timer-eligible, not scheduled by default |
| `fixture_evidence_corruption` | mutation of a copied record is detected | manual only |

Each profile declares its exact target class, hypothesis, steady state,
duration (15 or 30 seconds), single-fixture blast radius, abort thresholds,
rollback, cleanup checks, and expected evidence. The built-in adapters mutate
only in-process fixture state. There is no shell, command, argument-vector,
wildcard-target, host-service, network-wide, or sandbox-lifecycle executor.
Profiles longer than five minutes, missing safety declarations, manual-only
schedules, and targets not explicitly disposable or validation-designated are
rejected before a ledger write.

Inspect or run one profile:

```bash
python3 scripts/observability/activity_validation.py \
  --config configs/activity-validation.json \
  --evidence-config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  drill --profile fixture_backpressure \
  --target local-disposable-fixture --dry-run
```

Execution records start, threshold/duration abort when applicable, rollback,
cleanup, and terminal outcome. Rollback and cleanup are attempted even after an
executor failure. Their failures are explicit terminal states rather than
success. Host-wide, network-wide, production-target, or destructive experiments
remain manual and outside this runner.

## Timer schedule and cost bound

Invoke `schedule-once` from a systemd timer or equivalent. It reads durable
last-run state, runs at most `maximum_runs_per_invocation`, and exits; a second
invocation before the interval is a no-op. The reference schedule runs one
30-second-or-less local fixture per day. Together with the 15-minute canary
budget, this caps generated activity at 96 metadata events/day plus a few small
ledger records and avoids continuous compute allocation.

```bash
python3 scripts/observability/activity_validation.py \
  --config configs/activity-validation.json \
  --evidence-config /etc/agentic-sandbox/operational-evidence.json \
  --output-dir /var/lib/agentic-sandbox/evidence/operational \
  schedule-once
```

Recovery is bounded: stop further timer invocations, inspect the terminal drill
records, confirm rollback and cleanup, then re-run the same profile in dry-run.
If rollback or cleanup failed, keep the target out of service and recover the
disposable fixture manually; never widen the target to compensate. A failed
canary remains outstanding until its deadline and contributes failure evidence,
not organic availability.

## Tests

```bash
python3 -m unittest scripts/observability/test_activity_validation.py
```

The suite covers known-signal success and loss, rate/outstanding caps, unsafe
profile denial, all named profiles, dry-run isolation, threshold abort,
rollback/cleanup failures, timer state, and non-promotion into organic evidence.

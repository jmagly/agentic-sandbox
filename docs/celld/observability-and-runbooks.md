# Celld observability, recovery, and capacity

All Celld logs and traces carry `fleet_id`, `instance_id`, `generation`, `operation_id`, `trace_id`, `celld_version`, `adapter_version`, and node ID where applicable. Secret values, authorization headers, signed request headers, bundle contents, and object-store credentials are redacted.

The protected Titan profile installs `celld-live-observability`, but it refuses fault injection before mutation while any required dependency is absent. Current typed blockers are `CELLD_CREDENTIAL_PROVENANCE_AUTHORIZATION_REQUIRED`, `CELLD_ROLLOUT_QUALIFICATION_UNAVAILABLE`, and `CELLD_ALERT_TRACE_DASHBOARD_FIXTURE_UNAVAILABLE`. Even after those gates report ready, exact-run destructive authorization and a live adapter implementing the reviewed telemetry capture contract are mandatory. The controller persists an exact intent before every inject, heal, or emergency heal; validates a unique operation and W3C trace identity for every boundary; requires exact cross-surface identity agreement; and gives cleanup failure precedence over an operation error. Its deterministic tests and evidence schema do not promote UAT-CELLD-014. Live CLI, API, dashboard, log, trace, metric, and alert agreement remains `NOT_RUN` until the real fixture supplies every capture.

UAT-CELLD-014 promotion requires one independently applied and healed fault for
each exact boundary: Celld, management, store latency, store authorization,
store conditional semantics, provider, desired/observed divergence, unknown
effect, stale generation, and fleet below reserve. Every case must agree across
the CLI, API, dashboard fixture, logs, traces, metrics, and alert evaluator.
The trusted evaluator consumes the per-case captures rather than caller-supplied
totals. It requires one correlation record for every boundary/surface pair with
fleet, instance, generation, operation, W3C trace, Celld version, adapter
version, and node identities. Those identities must agree across every surface
for the same injected fault. It also requires a complete redaction scan,
exported evidence,
and a restored fleet baseline. CLI/API/dashboard repair presentations must be
labeled `plan` and must not claim that an effect occurred.

Required counters are commands accepted/replayed/rejected, effects pending/dispatched/unknown/terminal, reconciliation classifications, auth failures, stale-generation fences, alarms, bucket conditional failures, node membership, rolling-update phase, resident cells, RSS, CPU, storage bytes, and outbound requests. Histograms cover command acceptance, effect completion, reconciliation, bucket operations, Worker request duration, and alarm lateness.

Alerts distinguish at least:

- Celld unreachable while management is healthy;
- management unreachable while Celld continues serving durable intent;
- object-store latency, conditional failure, or authorization failure;
- desired/observed divergence older than five minutes;
- unknown effect outcome older than two retry intervals;
- stale-generation attempt (page immediately);
- fleet below reserve, resource ceiling over 85%, or incompatible rollout attempt.

Alert evidence contains injection, detection, heal, and resolution timestamps
for every required boundary. Divergence must be detected within five minutes,
an unknown effect within two recorded retry intervals, and a stale generation
within one recorded evaluation interval. Detection must occur while the fault
is active and resolution must occur only after the heal is applied. Aggregate
counts or a caller-authored `all_alerts_resolved` field cannot promote the gate.

`sandboxctl celld status`, `cell`, and `reconcile` provide operator diagnostics. A repair response is a plan, not proof an effect ran. Operators preserve and reuse original operation IDs.

## Recovery runbooks

**Celld node loss:** confirm other nodes and bucket health, replace from the pinned artifact, run diagnostics, wait for membership, reconcile. Repeating this procedure is safe because it does not mutate cell intent.

**Celld fleet unavailable:** stop new Celld-backed submissions, leave non-Celld runtimes operating, restore private networking/nodes, then reconcile every active generation. Do not replay commands blindly.

**Object store impaired:** freeze mutating Cell commands, retain management observations, repair storage semantics, validate conditional writes with a new preflight namespace, then reconcile. Never fail over to a store lacking proven conditional semantics.

**Unknown effect outcome:** query the management operation and current runtime inventory using the original operation ID. Mark terminal only with evidence; otherwise keep `unknown` and retry lookup, not the side effect.

**Disaster recovery:** restore a versioned bucket snapshot into an isolated prefix, verify manifests/digests and latest generations, start a quarantined fleet, compare against management inventory, fence stale generations, then change the broker reference. RPO target is the fleet manifest value (example: 300 seconds); RTO POC target is 30 minutes.

The Titan profile installs `celld-live-recovery`, but the driver starts no snapshot or restore mutation until all live recovery prerequisites exist. It returns separate pre-mutation blockers for credential/provenance authorization, observability qualification, a reviewed versioned-snapshot restore fixture, and evidence storage independent of the affected fleet. Even when those report ready, exact-run destructive authorization and the reviewed recovery adapter remain mandatory. The controller persists an exact intent before writer fencing, restore-authority changes, snapshots, restores, every runbook execution, external evidence uploads, affected-fleet loss/restoration, and fixture cleanup. It gives cleanup failure precedence over an operation error and requires the preflight baseline after emergency recovery. Local controller, ledger, schema, and hash tests remain supporting evidence only; they cannot promote UAT-CELLD-015 without real live observations.

UAT-CELLD-015 promotion requires two distinct versioned snapshots restored into
distinct isolated, quarantined prefixes while source writers are stopped and a
single restore authority is active. The evaluator derives RPO from the latest
acknowledged-state and snapshot timestamps, derives RTO from restore start and
ready timestamps, and compares generation and tombstone manifests before and
after each restore. Both restores must independently meet RPO <=300 seconds and
RTO <=30 minutes.

The node-loss, full-restart, authorization-loss, snapshot-restore, and
credential-rotation runbooks each execute exactly twice. Per-run operation IDs,
lifecycle-effect IDs, and resulting state hashes must be identical on the
second execution; a caller-supplied additional-effect count is not evidence.
Recovery evidence consists of the snapshot identity, restore timeline,
generation comparison, and evidence manifest stored under an authority distinct
from the affected fleet. Each artifact is downloaded after fleet loss, matched
to its SHA-256 digest, subjected to a detected corruption probe, and retained.
The evidence contract explicitly refuses a malicious-runner tamper-proof claim.

Quarterly exercises cover node loss, complete fleet restart, bucket authorization loss, snapshot restore, credential rotation, rollback, and incompatible rollout refusal. Capture timestamps and assertions; a prose-only walkthrough does not pass.

## Capacity and cost gates

Measure Celld compute, object-store requests/bytes, Worker CPU, resident memory/cells, telemetry, egress, and management reconciliation separately. The POC passes at 1,000 resident cells/node and 50 lifecycle operations/second only if p99 command acceptance is at most 250 ms, p99 convergence at most 30 seconds, duplicate effects are zero, and 24-hour error rate is below 1%. Production sizing keeps 30% CPU/memory headroom and one reserve node. Report monthly Celld fleet cost separately from QEMU, Docker, and host runtime costs.

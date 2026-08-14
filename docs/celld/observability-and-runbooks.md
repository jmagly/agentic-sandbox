# Celld observability, recovery, and capacity

All Celld logs and traces carry `fleet_id`, `instance_id`, `generation`, `operation_id`, `trace_id`, `celld_version`, `adapter_version`, and node ID where applicable. Secret values, authorization headers, signed request headers, bundle contents, and object-store credentials are redacted.

Required counters are commands accepted/replayed/rejected, effects pending/dispatched/unknown/terminal, reconciliation classifications, auth failures, stale-generation fences, alarms, bucket conditional failures, node membership, rolling-update phase, resident cells, RSS, CPU, storage bytes, and outbound requests. Histograms cover command acceptance, effect completion, reconciliation, bucket operations, Worker request duration, and alarm lateness.

Alerts distinguish at least:

- Celld unreachable while management is healthy;
- management unreachable while Celld continues serving durable intent;
- object-store latency, conditional failure, or authorization failure;
- desired/observed divergence older than five minutes;
- unknown effect outcome older than two retry intervals;
- stale-generation attempt (page immediately);
- fleet below reserve, resource ceiling over 85%, or incompatible rollout attempt.

`sandboxctl celld status`, `cell`, and `reconcile` provide operator diagnostics. A repair response is a plan, not proof an effect ran. Operators preserve and reuse original operation IDs.

## Recovery runbooks

**Celld node loss:** confirm other nodes and bucket health, replace from the pinned artifact, run diagnostics, wait for membership, reconcile. Repeating this procedure is safe because it does not mutate cell intent.

**Celld fleet unavailable:** stop new Celld-backed submissions, leave non-Celld runtimes operating, restore private networking/nodes, then reconcile every active generation. Do not replay commands blindly.

**Object store impaired:** freeze mutating Cell commands, retain management observations, repair storage semantics, validate conditional writes with a new preflight namespace, then reconcile. Never fail over to a store lacking proven conditional semantics.

**Unknown effect outcome:** query the management operation and current runtime inventory using the original operation ID. Mark terminal only with evidence; otherwise keep `unknown` and retry lookup, not the side effect.

**Disaster recovery:** restore a versioned bucket snapshot into an isolated prefix, verify manifests/digests and latest generations, start a quarantined fleet, compare against management inventory, fence stale generations, then change the broker reference. RPO target is the fleet manifest value (example: 300 seconds); RTO POC target is 30 minutes.

Quarterly exercises cover node loss, complete fleet restart, bucket authorization loss, snapshot restore, credential rotation, rollback, and incompatible rollout refusal. Capture timestamps and assertions; a prose-only walkthrough does not pass.

## Capacity and cost gates

Measure Celld compute, object-store requests/bytes, Worker CPU, resident memory/cells, telemetry, egress, and management reconciliation separately. The POC passes at 1,000 resident cells/node and 50 lifecycle operations/second only if p99 command acceptance is at most 250 ms, p99 convergence at most 30 seconds, duplicate effects are zero, and 24-hour error rate is below 1%. Production sizing keeps 30% CPU/memory headroom and one reserve node. Report monthly Celld fleet cost separately from QEMU, Docker, and host runtime costs.

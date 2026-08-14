# Celld qualification report - 2026-08-14

Upstream pin: Celld v0.2.1 / `ae8fac053d79f971bfcb996054bb43eb2f9b05da`
Agentic Sandbox implementation: `4d7e15a`
Architecture record: `32b091a`

## Evidence executed

| Evidence | Result |
|---|---|
| Full repository unit suite (`make test`) | PASS: management 913/913 plus management workspace, agent-rs, CLI 119/119, and CLI integration 1/1 |
| Rust formatting (`make lint`) | PASS |
| Focused Celld state/auth/ledger/validation suite | PASS: 16/16 |
| JSON contracts and examples parse | PASS |
| Reference Worker digest equals bundle and fleet manifests | PASS |
| Duplicate operation, request collision, stale generation, unknown outcome, restart-persistent effect claim | PASS in deterministic unit tests |
| Forged body, stale signed request, replayed nonce, zero generation | PASS in deterministic unit tests |
| Unsupported OS capabilities and hostile tenancy | PASS in deterministic negative tests |
| Unqualified rolling pair and incomplete object-store semantics | PASS in deterministic negative tests |

## Evidence not executed in this workspace

No dedicated Celld object-store account, private multi-node network, deployed v0.2.1 fleet, or production-like QEMU/Docker/host test inventory was supplied. Consequently these mandatory gates are NOT RUN:

- 100 live owner-loss/restart/partition trials on QEMU and Docker;
- host-substrate diagnostic trial;
- real object-store conditional-write races and latency distribution;
- public-to-internal network reachability test and proxy mutual-TLS exercise;
- three-node rolling update, rollback, reserve, and abrupt-node-loss exercise;
- live Worker API differential/negative/resource-limit suite;
- backup restore and credential-rotation incident exercises;
- 24-hour 50-operations/second soak and measured monthly cost model.

NOT RUN is not a pass and is not waived by the deterministic results.

## Support decisions

| Role | Experimental POC implementation | Production verdict | Reason |
|---|---|---|---|
| Durable InstanceCell orchestration (#749) | Available behind an off-by-default flag | **NO-GO** | Live provider, owner-loss, partition, and acknowledged-intent trials are not run. |
| Constrained worker-celld runtime (#750) | Reference bundle, discovery, validators, and examples available | **NO-GO** | Live Celld compatibility, differential API, and resource-ceiling evidence is not run. |
| Managed Celld fleets (#751) | Pinned manifests, service hardening, preflight, and rollout planner available | **NO-GO** | No multi-node deployment, network isolation proof, storage qualification, or rolling exercise exists. |

The optional adapter may be used only for controlled engineering experiments in a single trust domain. It is not a supported production runtime or control plane.

## Remaining-risk matrix

| Risk | Current control | Residual risk | Promotion evidence |
|---|---|---|---|
| Duplicate or lost lifecycle effect | operation/request binding, SQLite effect ledger, unknown-outcome lookup design | provider boundary is not exercised live | zero duplicates/loss in 10,000 repeats and 100 owner-loss trials |
| Stale destructive actor | generation checks and tombstones | real partition/failover behavior unproven | 100% pre-provider rejection during live partitions/reprovision |
| Internal API exposure | topology contract, TLS requirement, request authentication | firewall/proxy policy not deployed | external and cross-fleet reachability denial plus mutual-TLS exercise |
| Object-store split ownership | semantic preflight contract | provider-specific semantics/latency unknown | conditional-race and read-after-write evidence on the selected store |
| Service-material compromise | file delivery, permission check, scoped reference, rotation procedure | operational broker/proxy integration unexercised | rotation and evidence-redaction exercise with no disclosed value |
| Upstream alpha incompatibility | exact version/commit/digest and pair refusal | live surface drift unknown | pinned differential suite and successful rollback |
| Worker escape/resource exhaustion | denylisted capabilities and declared ceilings | runtime enforcement unproven | live negative and exhaustion suite without fleet impact |
| Recovery failure | runbooks, retention contract, tombstones | RPO/RTO unmeasured | independent restore evidence meeting 300-second RPO/30-minute RTO |
| Cost/capacity surprise | explicit counters and thresholds | no measured curve | 24-hour soak, 30% headroom, and separated monthly cost report |

Promotion requires a new report that replaces every relevant NOT RUN item with reproducible passing evidence and explicit security, operations, and test approval.

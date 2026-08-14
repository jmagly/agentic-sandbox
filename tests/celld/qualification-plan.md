# Celld integration qualification matrix

Run every row with Celld v0.2.1/`ae8fac0`, adapter `2026.8.3`, the recorded Worker digest, and each supported substrate. Preserve commands, timestamps, traces, metrics, and object-store evidence in the qualification report.

| Area | Automated assertion | Pass condition |
|---|---|---|
| Lifecycle | provision/start/stop/destroy plus duplicate delivery | one active generation and one effect per operation ID |
| Restart | kill owner after accept and before/after dispatch | acknowledged intent remains and converges |
| Timeout | drop response after management effect | result becomes unknown; lookup uses original operation ID; no second effect |
| Partition | isolate Celld-management, Celld-store, and one fleet node independently | diagnostic classification identifies the failed boundary |
| Fencing | deliver old-generation stop/destroy after reprovision | rejected; newer generation remains active |
| Object store | conditional-write races, stale reads, denied auth, latency | no split owner; preflight/alerts trip |
| Compatibility | current/current, unknown old/current, current/unknown | qualified pair rolls; unknown pairs are refused before drain |
| Worker JS | fetch/RPC/storage/alarm/WebSocket/outbound/Wasm/assets | declared supported capability works within limit |
| Worker negatives | exec, PTY, workspace, process, raw TCP, SSH, Docker, VM | discovery excludes and workload fails loudly |
| Resources | CPU loop, memory/storage/request/resident-cell excess | offending isolate throttled/terminated; fleet remains healthy |
| Security | forged MAC, stale timestamp, nonce replay, wrong generation, public internal route | all denied and observable; no effect |
| Resilience | rolling three-node update and abrupt node loss | reserve maintained, no acknowledged loss, convergence within 30 s |
| Disabled | unset `AGENTIC_CELLD_ENABLED` and run existing suites | prior QEMU/Docker/host behavior unchanged |
| Soak/cost | 24 hours at reference load | error <1%, p99 thresholds pass, quantified cost report produced |

Final reports make three separate decisions: durable InstanceCell orchestration (#749), constrained worker-celld runtime (#750), and managed fleet support (#751). A pass in one does not imply a pass in another. Any duplicate effect, lost acknowledged intent, stale destructive action, falsely advertised capability, secret leak, or public internal listener is an automatic no-go.

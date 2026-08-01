# Agent activity timeline PoC

Generated: 2026-08-01T17:29:34.896526Z

## Summary

- Events: 7
- Instances: 1
- Sessions: 1
- Sequence gaps: 1 (1 inferred missing events)
- Explicit dropped events: 1
- Chain head: `0575e7607cff946ffa5bc14729d3e20818be5f63c8483cc870fffd8c34c662a7`

## Timeline

| Time | Plane | Event | Source / trust | Correlation | Evidence |
| --- | --- | --- | --- | --- | --- |
| 2026-08-01T12:00:00.000Z | session | session.started | management-events / attested | instance-demo/session-demo | metadata recorded |
| 2026-08-01T12:00:00.100Z | action | agent.tool.invoked | provider-adapter / self-reported | instance-demo/session-demo/tool-demo | tool_name=shell |
| 2026-08-01T12:00:00.120Z | action | process.exec | guest-exec-observer / observed | instance-demo/session-demo/tool-demo | executable=/usr/bin/curl |
| 2026-08-01T12:00:00.140Z | network | network.flow | runtime-flow-observer / observed | instance-demo/session-demo/tool-demo | destination=198.51.100.20:443, protocol=tcp |
| 2026-08-01T12:00:00.150Z | runtime | runtime.resource.sample | runtime-cgroup-sampler / observed | instance-demo/session-demo | cpu_percent=3.25, memory_bytes=67108864 |
| 2026-08-01T12:00:00.200Z | action | process.exited | guest-exec-observer / observed | instance-demo/session-demo/tool-demo | exit_code=0 |
| 2026-08-01T12:00:00.201Z | integrity | telemetry.loss | guest-exec-observer / observed | instance-demo/session-demo | reason=queue_overflow |

## Loss report

- Collector `guest-exec-observer` is missing 1 event(s) between sequence 1 and 3.
- Collectors explicitly reported 1 dropped event(s).

## Interpretation

This output proves correlation and loss accounting for a bounded fixture. The hash chain detects post-normalization mutation but does not prove that a source collector was complete or truthful; production collectors need separate keys or an append-only trusted sink.

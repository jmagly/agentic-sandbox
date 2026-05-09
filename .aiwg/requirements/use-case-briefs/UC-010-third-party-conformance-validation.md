# UC-010: Third-Party Orchestrator Validates Compliance via Conformance Harness

## ID

UC-010

## Primary Actor

Third-party orchestrator integrator (e.g. someone building a smolagents↔agentic-sandbox adapter, or a custom internal orchestrator)

## Stakeholders

- **Third-party integrator**: needs to verify their integration is correct without reading every spec word.
- **agentic-sandbox project**: gains a verifiable claim of compliance by other implementations.
- **AIWG**: confirms its own implementation by running the same harness in CI.
- **Future AIWG-compatible orchestrator authors**: have a "did I do it right?" tool.

## Goal

Any orchestrator that claims AIWG-executor compatibility can run the published conformance harness against their integration target (an agentic-sandbox v2 instance) and receive a pass/fail report covering ~50 normative requirements.

## Pre-conditions

- Conformance harness binary published for relevant platform: `agentic-sandbox-conformance-v2.0.0-linux-amd64` (or container image `ghcr.io/roctinam/agentic-sandbox-conformance:v2.0.0`).
- A running agentic-sandbox v2 instance reachable from the harness execution environment.
- Bearer token for the running sandbox (or mTLS cert in v2.1+).

## Main Flow

1. Integrator downloads the harness binary matching the spec version they target:
   ```
   curl -L -o agentic-sandbox-conformance \
     https://git.integrolabs.net/roctinam/agentic-sandbox-conformance/releases/download/v2.0.0/agentic-sandbox-conformance-linux-amd64
   chmod +x agentic-sandbox-conformance
   ```
2. Integrator runs harness against their target sandbox:
   ```
   ./agentic-sandbox-conformance \
     --executor-url http://my-sandbox:8122 \
     --token "$AIWG_BEARER_TOKEN" \
     --report-format markdown,junit \
     --report-out conformance.report.md
   ```
3. Harness:
   - Probes `GET /api/v2/` to confirm v2 spec version match.
   - Runs the ~50 tests listed in ADR-010 in dependency order.
   - Emits progress to stderr; final report to stdout / file.
4. Harness exits with code 0 (all pass), 1 (failures), or 2 (configuration error).
5. Integrator inspects report:
   - Markdown report shows per-category pass/fail with diffs of expected-vs-actual for failures.
   - JUnit XML can be ingested by CI (Jenkins, GitHub Actions, GitLab) for trend tracking.
6. Integrator iterates on their integration; reruns harness; achieves clean pass.

## Alternative Flows

### A1. Sandbox claims v2.0 but harness detects v1.x behavior

- Discovery probe returns version mismatch.
- Harness exits with code 2 + "Spec version mismatch: target reports v1.0.0; harness expects v2.0.0".
- Integrator either upgrades sandbox or downloads matching-version harness.

### A2. Integrator's WS adapter doesn't honor `?since=<seq>` resume cursor

- Test "Reconnect with cursor replays missed events" fails.
- Markdown report shows: "Expected events 5..10 to replay on reconnect; got events 8..10. Missing: 5,6,7."
- Integrator fixes adapter; reruns; passes.

### A3. Integrator targets v2.x with v2.0 harness

- Discovery probe returns v2.1; harness is v2.0.
- Harness runs with warning: "Target is newer than harness. Tests cover v2.0 surface only; new v2.1 behavior is not validated here."
- Exits 0 if v2.0-equivalent surface passes.
- Integrator may also run the v2.1 harness for full coverage.

### A4. Sandbox is multi-tenant (v2.2+) and harness needs tenant context

- Harness flag `--tenant <tenant_id>` provided.
- Tests run scoped to that tenant; cross-tenant tests skipped if only one tenant accessible.

### A5. Restart resumability test requires side-channel control of sandbox

- Harness flag `--restart-cmd "kill -9 $(pgrep agentic-sandbox-mgmt)"` provided.
- Test triggers restart, waits for sandbox to come back, verifies recovery.
- If `--restart-cmd` absent, test skipped with warning: "Restart resumability not validated."

### A6. Network conditions affect timing-dependent tests

- Heartbeat test, idempotency-cache TTL test have wall-clock dependencies.
- Harness flag `--timing-tolerance ms` allows slack; default 500ms.
- High-latency environments may need higher tolerance.

## Post-conditions

- Conformance report exists in known format(s).
- Integrator knows exactly which spec requirements their target meets and which fail.
- For agentic-sandbox CI, report is uploaded as artifact and pass/fail gates the merge.
- For third-party CI, integrators publish report results to their own dashboards / docs.

## Acceptance Criteria

- AC-1: Single `agentic-sandbox-conformance --executor-url X --token Y` invocation runs the full v2.0 test suite.
- AC-2: Exit code 0 if all tests pass; non-zero with informative output otherwise.
- AC-3: Markdown report is human-readable; JUnit XML is machine-readable.
- AC-4: Tests are idempotent: running the harness against the same target twice in a row yields the same pass/fail result, with no leftover side effects (mission counts, idempotency cache pollution).
- AC-5: Tests are platform-independent: same harness binary works against linux-amd64, linux-arm64, darwin, windows targets.
- AC-6: agentic-sandbox CI pipeline has "conformance" check that runs harness against an in-CI sandbox; failure blocks merge.
- AC-7: AIWG CI pipeline can run the same harness against an agentic-sandbox v2 instance and use it as a release gate.
- AC-8: Harness source code is MIT/Apache licensed and publicly hosted.
- AC-9: Spec changes that break harness tests trigger explicit harness version bump (so old harnesses against newer specs fail clearly).

## Related

- ADR-010 (conformance harness as published executable)
- ADR-006 (harness reads canonical schemas)
- Vision §4 success criteria S2, S3, S10
- Risk R-4 (scope creep; harness scope bounded to ADR-010)
- Best-practices research §6 (OCI Distribution conformance precedent)

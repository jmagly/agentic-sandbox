# Celld qualification delivery roadmap

## Purpose

This roadmap closes Gitea issues #761 and #763 through #772 in dependency
order, then supplies the evidence needed to resolve the #752 security and #754
qualification epics. It automates UAT-CELLD-003 through 015 on Titan while
leaving UAT 016 soak and UAT 017 human acceptance operator-run.

The work adds an optional S3-compatible backing store for explicitly enabled
Celld fleets. It does not replace or migrate local filesystems, workspaces,
agentshare, VM disks, bind mounts, container volumes, or management state.

Live campaigns are medium-blast-radius operations on a shared host: they create
and destroy exact-run disposable Docker, libvirt, network, credential, and
object-store resources. They require operator authorization, a structural
dry-run, exact ownership checks, and capacity-one Titan serialization.

Workflow dispatch is intentionally non-idempotent: every dispatch creates a
new run ID and new disposable resources. Do not repeat a dispatch until its run
identity, verdict, evidence, and cleanup are known. Exact-run cleanup, janitor
preview, and absence verification are designed to be idempotent.

## System topology

- Repository: `roctinam/agentic-sandbox`, protected evidence from exact signed
  commits on `main`.
- Execution host: `titan`, selected by the `titan` runner label.
- Runner: `titan-host-runner`, capacity `1`.
- Storage candidate: SeaweedFS 4.41 linux/amd64 manifest
  `sha256:3bbe24f6d5f5818327adcfeda7d85240ed53212dab05f91af14484c6446ec5eb`.
- Approved Celld lane: v0.2.1. Reviewed v0.3.0 remains unqualified and must not
  be promoted from a smoke run.
- Live storage root: `/dev/shm/agentic-celld-storage`, exact-run ownership only.
- Shared resource reservations:
  `agentic-sandbox-celld-qualification-titan` and
  `agentic-sandbox-vm-e2e`, both non-cancelling.
- Evidence retention: Gitea artifacts retained for 90 days, with verified
  manifests and exact commit/backend/topology identities.
- Operator prerequisites: authenticated `tea` 0.14.1 access to Gitea, a clean
  Git checkout with `origin` configured, and `git`, `jq`, and `node` available.

Authoritative requirements are the 44-assertion RTM and test strategy in the
adjacent AIWG corpus. The executable catalog and implementation contract live
in `tests/celld/uat/scenarios.json` and `tests/celld/uat/README.md`.

## Current delivery checkpoint

The 2026-08-23 implementation audit changed the plan from "run the existing
drivers" to "close their evidence-integrity gaps, then run them." Static or
caller-authored values must not be promoted merely because the aggregate
evaluator accepts their shape.

| Work | State | Next promoting gate |
|---|---|---|
| #759–#762 | Closed foundation. | Preserve their exact evidence and optional-S3 boundary. |
| #763 | Credential-launcher repair is delivered; the one failed live attempt was diagnosed and cleaned. | Exact-head required CI, manual mutsu Docker availability, then one approved-channel fleet retry with Titan idle. |
| #764 | Implemented but not yet live-qualified. Exact-run mutation ownership, provider-state joins, durable dispatch counts, and the UAT-006 runtime scope repair are delivered. UAT-004 now uses an owner-only, operation-bound management dispatch gate and independently kills management plus the observed Celld owner for every trial; evaluator-owned formulas reject reused fault identities, grouped trials, phase/PID mismatch, invalid timestamp order, missing epoch advance, provider drift, or incomplete heal. | Obtain exact-head focused/CI evidence, then run the issue-scoped UAT-003–006 Titan lane only after #763 and the capacity gate are green. |
| #765 | Implementation complete; exact-head live evidence pending. The driver installs crash-recoverable listener guards before exposing per-node private mTLS proxies, routes management through the exact private-CA leaf, retains 9,000 raw UAT-012 attempts across nine distinct classes, and rejects wrong SAN/CN/root, expired, cross-fleet, environment-proxy, and plaintext-bypass cases before provider effects. UAT-010 now records all 3,000 forbidden-route attempts plus before/during/healed matrices for management-to-Celld, Celld-to-management, Celld-to-store, and node-to-peer partitions. Every nftables table, proxy, namespace, and probe is exact-run inventoried and independently checked absent. | Obtain exact-head CI, then run the issue-scoped UAT-010/012 Titan lane and retain the schema-valid artifacts before closure. |
| #766 | Controller and evidence-contract implementation is complete; the live Titan adapter remains blocked. The controller persists 11 exact intents before mutation, requires five distinct credential lifecycles, four bidirectional cross-fleet bucket cases, four field-specific provenance mismatch verifiers, seven inventory-bound leakage scans, and cleanup-failure precedence. UAT-013 evaluators remain candidate-only. Upstream Celld v0.3.0 still stores one internal peer secret and exposes no supported rotation API, so direct bucket editing is forbidden and cannot be promoted. | Implement only reviewed live activation mechanisms. If peer-secret rotation remains unavailable, emit a typed pre-mutation `NOT_RUN`; do not enable the evaluator or dispatch the credential campaign until all five lifecycle mechanisms and exact cleanup are reviewable. |
| #767 | Implementation complete; exact-head live evidence pending. Candidate and approved deployments share the protected, inventoried credential launcher and retain exact source/Worker/version identities. Rollback compares an observed durable nonce marker before and after restoration. UAT-008 retains 800 bounded typed rejection records and independent process/file/socket/container/VM inventories. UAT-009 remains a pre-mutation `NOT_RUN`, and its six unavailable enforcement families are not advertised as capabilities. | Obtain exact-head CI, then run the issue-scoped UAT-007/008 Titan lane and retain the schema-valid artifacts before closure. |
| #768 | The deterministic rolling controller and evidence formulas are implemented, including one-node budgets, reserve checks, persist-before-mutate intents, abrupt-node rebuild, threshold/emergency rollback, approved-digest restoration, and pre-drain incompatible-pair refusal. | Keep mutation `NOT_RUN` until v0.3.0 passes exact-head approved/candidate fleet qualification, is separately approved as a compatible immutable pair, and a reviewed Titan substrate adapter is installed. |
| #771 | Implementation complete; exact-head live evidence pending. Every mutation and cutover is persisted in a protected hash-chained journal; authority observations join both bucket write policies with exact writer/fleet state; forward and reverse evidence retain separate key-set, size, content, metadata, and combined hashes; the reviewed Worker is deployed to both stores. Post-journal failures write schema-bound evidence and retain both namespaces stopped and write-denied for exact operator recovery. | Obtain exact-head CI, then run the issue-scoped two-store Titan rehearsal and retain the success/error schemas, journal, artifact manifest, and cleanup/postflight evidence before closure. |

The active dual-track iteration is therefore:

1. **Delivery track:** close #763 only after the exact-head CI and operator host
   prerequisite gates are green. Do not overlap its retry with construction
   campaigns.
2. **Construction track:** land #764's exact-run orchestration inventory. Every
   provider resource and fault target is persisted before mutation, the
   repository/workflow/run/host identity is bound, and all four scenarios need
   explicit destructive authorization. This creates the crash-recovery seam;
   it does not make #764 dispatchable by itself.
3. Validate the delivered operation-bound UAT-004 dispatch gate, independent
   per-trial management/owner faults, raw phase timelines, provider joins, and
   evaluator-owned negative cases at the exact signed head. Only then run the
   existing issue-scoped protected Titan lane.
4. Validate the delivered #765 private proxy, listener guard, nine-class
   denial matrix, and four directional partition matrices, then #767's credential and
   observation repair and #771's durable cutover journal. The protected
   workflow has strict issue selections so one lane cannot suppress another's
   evidence; those selections remain non-promoting until their implementation
   gates are complete.
5. Wave 4 now builds only on the accepted Wave 3 interfaces. Its controller and
   evidence contracts do not promote a live verdict until their real Titan
   adapters and upstream lifecycle mechanisms are independently available.

## Procedure

### Delivery gates

Every issue passes the same Definition of Done:

1. Its complete acceptance contract is present in Gitea and traceable to the
   RTM.
2. Focused structural tests pass on Titan at the exact candidate commit.
3. Required live evidence is derived from raw observations; caller-authored
   summaries, reduced fixtures, and static checks cannot satisfy live gates.
4. Cleanup proves exact absence. Residue, ambiguous ownership, interruption,
   or missing evidence is `ERROR` with exit 4.
5. Affected regression and CI lanes are green at the same commit.
6. The issue records the run and artifact evidence before closure.

### Wave 0 — restore trustworthy current-head signals

**Scope:** current `main`, CI policy, #761 readiness.

1. Confirm a clean, pushed head and an online, idle capacity-one Titan runner.
2. Confirm no `agentic-sandbox` run is queued or active before dispatch.
3. Use the storage workflow's bounded deterministic step as the dry-run. It
   executes the storage qualifier, fixture tests, and UAT contract check on
   Titan before creating the live fixture.
4. Before release, remove CI signal suppression that can turn security scan
   failures into success (`continue-on-error`, forced zero exit, or broad
   `|| true`) and eliminate unreviewed floating runtime images.

### Wave 1 — qualify the storage substrate and start #766 TDD

**Issue:** #761. **Parallel code track:** #766 contract and driver.

1. Dispatch exactly one storage campaign:

   Record a fresh approval checkpoint in #761 before running the command. The
   checkpoint must state: medium blast radius; non-idempotent dispatch; exact
   `origin/main` SHA; Titan idle/online; zero active project runs; and the
   fail-closed preflight/deterministic dry-run that precedes live mutation.

   ```sh
   tea actions workflows dispatch celld-storage-qualification.yml \
     --repo roctinam/agentic-sandbox \
     --ref main
   ```

   Expected CLI output: `Workflow dispatched successfully`; exit status 0. The
   post-dispatch API verification must then show one new serialized
   `celld-storage-qualification.yml` run for the exact `origin/main` commit. Do
   not retry until the run identity, verdict, evidence, and cleanup have been
   inspected.

2. Credit #761 only when the run proves:

   - `CELLD.010.DETERMINISTIC_PREFLIGHT=PASS`;
   - `CELLD.010.STORAGE=PASS` and the separately owned isolation assertion
     remains truthful `NOT_RUN`;
   - 10,000 create and 10,000 overwrite rounds reconcile exactly;
   - cross-gateway reads and all four latency families meet p99 <=250 ms;
   - invalid, expired, wrong-bucket, and cross-bucket access is denied;
   - the exact named `titan-single-host-storage` topology, immutable backend
     pin, TLS endpoints, and CPU/memory/PID ceilings match the reviewed profile;
   - the unpredictable run bucket uses a least-privilege identity distinct from
     the fixture administrator, with shared-prefix IAM still `NOT_RUN`;
   - the evidence manifest verifies and exact-run cleanup leaves zero residue.

3. Repair #766's truncated tracker acceptance criteria from the canonical issue
   plan, then add failing contract/evaluator tests for the fixed
   `celld-live-credential-provenance` driver. Do not remove the current
   `WITHHELD` formulas until the driver, raw evidence, and cleanup contract are
   implemented.

### Wave 2 — establish the shared three-node substrate

**Issue:** #763. **Depends on:** #761 and closed #759/#762.

1. Add a focused Titan step for the fleet fixture, preflight, workflow, and
   contract tests before live mutation.
2. Dispatch the approved lane:

   Record a fresh approval checkpoint in #763 before running the command. State
   the medium blast radius, non-idempotent run creation, exact SHA, idle checks,
   and passing focused dry-run evidence.

   ```sh
   tea actions workflows dispatch celld-fleet-fixture.yml \
     --repo roctinam/agentic-sandbox \
     --ref main \
     --input celld_channel=approved
   ```

   Expected CLI output: `Workflow dispatched successfully`; exit status 0. API
   verification must show one new exact-head `celld-fleet-fixture.yml` run
   before any result receives credit.

3. Require three exact nodes with immutable Celld/application pins, one
   reserve, private routes, readiness and membership, instrumented QEMU/Docker
   boundaries, CLQ-04 preflight/diagnose observations, Worker deployment/probe,
   three callback relays, inventory persistence before every creation,
   start/end janitor previews, termination-boundary coverage, two successful
   cleanup passes, and an independent zero-residue sweep.

Evidence must say `single-host multi-node`; it cannot claim physical-host,
rack, or availability-zone resilience.

### Wave 3 — execute independent qualification lanes

Run these serially after #763; a failure in one lane does not erase the verdict
or evidence from another:

| Issue | Gate |
|---|---|
| #764 | UAT 003–006 replay, crash, response-loss, stale/future-generation, provider-effect, and p95 convergence matrices. |
| #765 | Full listener/certificate/bypass matrix and directional management/store/peer partitions with healed-state cleanup. |
| #767 | UAT 007/008 live Worker capability, rollback, loud rejection, and zero-host-effect evidence. |
| #771 | Two-store quiesced migration, destination canary, pre-write rollback, post-write reverse migration, single authority, hashes, and cleanup. |

The protected workflow accepts only the reviewed `complete`, `issue-764`,
`issue-765`, `issue-767`, and `issue-771` selections. `complete` remains the
default, runs the migration, and selects all UAT 003–015. Issue selections run
only their catalog drivers or the #771 migration. A partial run may close only
its selected issue and must retain the `partial-selection` label; it cannot
claim consolidated qualification. Do not dispatch an issue selection until
the corresponding implementation gate above is complete.

Pinned Celld v0.2.1 cannot pass UAT-009 because it lacks all six required
per-isolate resource controls. Do not weaken the RTM or advertise unverified
capabilities. Either qualify a newer immutable Celld pin that exposes the
controls or record the #750 support decision as experimental/deferral.

### Wave 4 — retire substantive implementation blockers

1. **#766 credentials and provenance:** implement the fixed driver, five-domain
   lifecycle inventory (S3, request HMAC, mTLS, peer secret, fixture admin),
   bidirectional cross-fleet scope matrix, bounded rotation/revocation,
   provenance mismatch matrix, independent leakage scans, and cleanup.
2. **#768 rollout:** qualify a distinct old/new immutable pair and implement the
   Titan adapter before enabling mutation. Prove one-node availability,
   untouched reserve, node-loss rebuild, threshold rollback, restored digests,
   p95 convergence, and incompatible-pair refusal before drain.
3. Qualify peer-secret rotation/recovery through a reviewed controlled restart
   when live reload is unsupported. A detected absence of any safe upstream
   mechanism remains typed pre-mutation `NOT_RUN` and blocks the corresponding
   support claim; never simulate rotation or edit bucket state ad hoc.

### Wave 5 — observability and recovery

1. **#769:** build the live dashboard/log/trace/metric/alert fixture. Inject all
   ten required boundaries and require agreement across seven surfaces,
   correlation identities, timing limits, redaction, alert resolution, and a
   healed baseline. The persist-before-mutate controller, exact 10x7 evidence
   contract, alert lifecycle validation, and cleanup path are implemented; the
   real dashboard/log/trace/metric/alert adapter remains a typed pre-mutation
   prerequisite and no live assertion is promoted without it.
2. **#770:** build versioned snapshot and isolated-restore fixtures plus an
   evidence sink outside the affected fleet. Execute all five runbooks twice,
   measure RPO <=300 seconds and RTO <=30 minutes, prove generation/tombstone
   equality and second-run idempotency, then read and hash-verify evidence after
   fleet loss. The persist-before-mutate recovery controller, two-restore/five-
   runbook/four-evidence contract, corruption probes, and cleanup path are
   implemented; the versioned snapshot and independent evidence-store adapter
   remains a typed pre-mutation prerequisite.

### Wave 6 — consolidated qualification and release

**Issue:** #772. **Parents:** #752 and #754.

1. Run a static readiness gate that proves all fixed drivers exist, all 44
   assertion IDs have one owner/formula, start/end janitors are present, no
   required live profile remains disabled, and every dependency has acceptable
   exact-head evidence.
2. Dispatch the protected workflow only after readiness passes:

   Record a fresh approval checkpoint in #772 before running the command. State
   the high blast radius from destructive fault injection, non-idempotent run
   creation, exact SHA, passing readiness/dry-run evidence, idle checks, and
   the exact-owned cleanup/recovery path.

   ```sh
   tea actions workflows dispatch celld-qualification.yml \
     --repo roctinam/agentic-sandbox \
     --ref main \
     --input qualification_lane=complete \
     --input allow_destructive_faults=true
   ```

   Expected CLI output: `Workflow dispatched successfully`; exit status 0. API
   verification must show one new exact-head `celld-qualification.yml` run
   before any evidence receives credit.

3. Require 13 scenario records for UAT 003–015, all 44 assertions exactly once,
   independent lane verdicts, 480/420-minute ceilings, verified manifests,
   postflight resource parity, at most 10 GiB retained evidence, and zero
   Docker/libvirt/VM/agentshare/fault residue. Upload and verify evidence for
   every PASS, FAIL, NOT_RUN, ERROR, interruption, and cleanup-failure path.
4. Resolve #752/#754 only with explicit security residual risks, support matrix,
   performance/cost baselines, and separate support decisions for #749, #750,
   and #751.
5. Run release CI at the exact candidate head, then create and publish the
   release only from green required gates. UAT 016 soak and UAT 017 human
   acceptance remain separate operator evidence and are never synthesized.

## Verification

Before every dispatch:

```sh
WORKFLOW_FILE="celld-storage-qualification.yml"
git fetch --prune origin main
git status --short
test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" && echo HEAD_SYNCED
tea api 'admin/actions/runners?limit=100' \
  | jq -e '.runners[] | select(.name == "titan-host-runner" and .status == "online" and .busy == false) | {name,status,busy}'
tea api 'repos/roctinam/agentic-sandbox/actions/runs?limit=100' \
  | jq -e '[.workflow_runs[] | select(.status == "waiting" or .status == "queued" or .status == "running" or .status == "in_progress")] | if length == 0 then {active: 0} else error("project run already active") end'
BEFORE_RUN_ID="$(
  tea api 'repos/roctinam/agentic-sandbox/actions/runs?limit=100' \
    | jq --arg workflow "${WORKFLOW_FILE}" \
        '[.workflow_runs[] | select(.path | startswith($workflow + "@")) | .id] | max // 0'
)"
printf 'BEFORE_RUN_ID=%s\n' "${BEFORE_RUN_ID}"
```

Expected: no worktree output, `HEAD_SYNCED`, Titan `online`, and zero waiting
or active project runs before a new campaign is submitted. The final line is a
numeric `BEFORE_RUN_ID`. Set `WORKFLOW_FILE` to the exact workflow about to be
dispatched. `git fetch` updates only remote-tracking refs; inspect and resolve
any divergence before dispatch.

After dispatch:

```sh
EXPECTED_SHA="$(git rev-parse origin/main)"
tea api 'repos/roctinam/agentic-sandbox/actions/runs?limit=100' \
  | jq -e --arg sha "${EXPECTED_SHA}" \
      --arg workflow "${WORKFLOW_FILE}" \
      --argjson before "${BEFORE_RUN_ID}" '
      [.workflow_runs[]
       | select(.event == "workflow_dispatch")
       | select(.id > $before)
       | select(.head_sha == $sha)
       | select(.path | startswith($workflow + "@"))]
      | sort_by(.id)
      | last
      | {id, status, conclusion, head_sha, path, html_url}
    '
```

Expected: one object whose `head_sha` equals `EXPECTED_SHA` and whose `path`
starts with `celld-storage-qualification.yml@`, with an `id` greater than
`BEFORE_RUN_ID`. Set `WORKFLOW_FILE` to `celld-fleet-fixture.yml` or
`celld-qualification.yml` for those dispatches. A missing/newer-object failure
or different commit receives no credit.

Final closure requires links to the exact run, verified artifact manifest,
assertion-level results, cleanup/postflight result, and supporting CI runs in
each Gitea issue.

## Troubleshooting

- **Titan is busy:** leave the run queued. Do not enable a second runner,
  increase capacity, reroute to the workstation, or cancel another job.
- **Preflight returns `NOT_RUN`:** resolve the named prerequisite before its
  mutation. Do not convert prerequisite absence into PASS.
- **Cleanup exits 4:** preserve inventory and evidence, inspect exact-run
  resources, and repair scoped cleanup. Never run a global Docker/libvirt
  prune.
- **Run SHA differs from `origin/main`:** do not use the evidence. Wait for the
  current run to clean up, then dispatch once at the correct head.
- **UAT-009 remains `NOT_RUN`:** inspect a newer immutable Celld release or
  record the required experimental/deferral decision. Do not weaken limits.
- **#768 candidate fails provenance or startup:** retain
  `reviewed_unqualified`; fix the exact incompatibility before approving a
  pair.
- **Evidence or manifest is missing:** classify the scenario `ERROR`, retain
  available diagnostics, and do not rerun until exact cleanup is proven.

## House Rules for Agents

- Do run resource-intensive tests only on Titan and keep capacity at one.
- Do use exact signed commits, immutable artifact identities, and raw evidence.
- Do stop on ownership, provenance, redaction, cleanup, or manifest mismatch.
- Do post Gitea progress comments at every implementation and live-evidence
  checkpoint.
- Do keep secrets in protected tmpfs files or descriptors and out of argv,
  captured environments, logs, artifacts, and issue comments.
- Do not overlap Celld campaigns or shared VM E2E.
- Do not promote support-only, deterministic, reduced-fixture, or candidate
  smoke evidence into a live hard gate.
- Do not create, mutate, or delete non-disposable operator storage.

## What NOT to Fix

- Local filesystem and volume behavior remains the default by design.
- SeaweedFS qualification is exact-version/topology evidence, not vendor
  lock-in or a claim about every S3-compatible provider.
- `single-host multi-node` is intentionally narrower than multi-host evidence.
- Shared-prefix IAM remains `NOT_RUN` unless independently qualified.
- Capacity one and non-cancelling cleanup are safety controls, not performance
  bugs.
- UAT 016 soak and UAT 017 human acceptance remain operator-run.
- A truthful unsupported capability is `NOT_RUN`; it must not be changed to
  PASS to make the consolidated workflow green.

## Audit Trail

- 2026-08-16: canonical issue plan, test strategy, use case, and 44-assertion
  RTM approved and filed as Gitea #759–#772.
- 2026-08-21: implementation advanced through current `main`, but host overload
  paused execution before current-head CI and live evidence.
- 2026-08-22: Codex with Agentic Sandbox maintainers — dependency/readiness,
  test-plan, and #766 security-gap audits consolidated into this delivery
  roadmap. Applicable live host: Titan only.
- 2026-08-23: Wave 3 implementation audit found non-promoting evidence gaps in
  #764/#765/#767/#771. The first #764 construction slice was started at the
  destructive-ownership boundary; none of these issues is authorized for live
  credit yet.
- Last audited parent baseline: 2026-08-23 against signed `main` at
  `ac027fb00104c3c1fcf5fc52219ed7c0126741c6`; the resulting construction
  commit still requires exact-head CI. Live qualification remains pending.

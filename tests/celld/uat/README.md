# Celld structured UAT

`scenarios.json` is the versioned acceptance catalog for issues #747-#754.
Scenarios 001-015 have the `automated` trigger. The actual 24-hour campaign
(016) and representative-user session (017) are explicitly operator-triggered.
The dependency-free runner executes only hardcoded deterministic executor IDs;
the catalog cannot inject shell commands. Automated live scenarios 003-015
run a shared credential-free qualification suite and attach its narrowly
scoped supporting assertions to each scenario. The executor is cached, so the
suite runs once per catalog invocation rather than once per scenario. A live
scenario remains `NOT_RUN` until its disposable environment and driver supply
the required hard-gate evidence; supporting unit evidence never promotes it.

Live automation uses two strict JSON contracts:

- `live-profile-v1.schema.json` identifies the exact run, commit, single-host
  scope, destructive authorization, inventory file, and enabled fixed drivers.
- `live-observation-v1.schema.json` is the only accepted driver output. Drivers
  emit measurements, identities, faults, artifact hashes, prerequisites, and
  cleanup observations; they cannot emit verdicts or commands.
- `storage-profile-v1.schema.json` and `storage-evidence-v1.schema.json` define
  provider-neutral S3-v1 inputs and raw measurements. Reduced fixture evidence
  is always non-promoting; GCS is a typed `NOT_RUN` reservation.
- `seaweedfs-fixture-v1.schema.json` binds the first self-hosted candidate to
  its exact image manifest, named topology, run directory, resource ceilings,
  bucket scope, and protected identity-file references.
- `live-orchestration-v1.schema.json` confines UAT 003-006 to the exact Titan
  run, reviewed libvirt and storage roots, job-scoped agentshare, immutable
  local Docker image ID, fixed management binary, and static callback relay.

The three-node Titan fixture is managed by
`scripts/celld-fleet-fixture.mjs`. Its crash-resumable inventory is persisted
before every resource mutation, labels the environment `single-host
multi-node`, publishes only Worker listeners on host loopback, and keeps the
unauthenticated Celld internal listener unpublished. It does not change the
operator-run boundary for UAT 016 and 017.

`scripts/celld-live-orchestration.mjs` supplies the real QEMU/Docker lifecycle
and provider-fault evidence for UAT 003-006. Each scenario creates its own
S3-backed three-node fleet, starts management on the private Docker bridge with
required mTLS, keeps the callback certificate out of the admin allowlist, and
records only hashed operation identities and sanitized measurements. UAT 003
replays each lifecycle effect 10,000 times and injects hash collisions. UAT 004
performs owner/management crash campaigns before, during, and after dispatch.
UAT 005 arms an explicit one-shot post-effect response loss for every trial.
UAT 006 pauses the callback relay while stale and future generations are
fenced, then verifies the active provider checksum after healing. Fault
scenarios require both workflow opt-in and exact run ownership before any
fixture mutation.

The catalog assigns each live assertion to one hardcoded driver ID. Driver
programs and timeouts live in `scripts/celld-uat-live-protocol.mjs`; catalog and
profile data cannot choose a program. The runner uses `shell:false`, a bounded
environment/output/timeout, exact scenario/run joins, trusted evaluator
functions, and post-write manifest verification. An unavailable prerequisite
is `NOT_RUN` only before mutation. Missing/corrupt evidence, timeout,
interruption, identity mismatch, or setup failure after mutation is `ERROR`.
Cleanup residue is exit 4 regardless of assertion observations.

Trusted formulas for the authorized live work are centralized in
`scripts/celld-live-evaluators.mjs`. They derive lifecycle counts, recovery
percentiles, capability completeness, containment, topology denial, rollout
budgets, alert timing, and recovery objectives from raw measurements; a driver
cannot supply its own verdict field. The three UAT-013 credential/provenance
formulas remain intentionally unregistered until the separate #766 threat gate
is explicitly authorized.

## Degraded-mode and evidence matrix

| Condition | Derived result | Mutation rule |
|---|---|---|
| No live profile, disabled driver, missing registered driver, or unavailable prerequisite detected before setup | `NOT_RUN` / exit 2 | No mutation may start. |
| Supporting deterministic gate fails | `FAIL` / exit 1 | Live driver is not invoked. |
| Invalid profile or pre-launch commit/run mismatch | `ERROR` / exit 3 | No mutation may start. |
| Driver crash/timeout or invalid JSON after launch | `ERROR` / exit 4 | Cleanup cannot be proven and is conservatively failed. |
| Observation identity mismatch, missing assertion, evaluator failure, or missing/tampered artifact with proven cleanup | `ERROR` / exit 3 | Preserve observations and cleanup proof. |
| Trusted evaluator observes a threshold or invariant violation | `FAIL` / exit 1 | Cleanup still runs and the failure remains recorded. |
| Any cleanup assertion fails | `ERROR` / exit 4 | Cleanup failure outranks every other result. |

The corruption/completeness threat model is fail-closed: strict catalog fields
block command injection; observation schemas forbid verdicts; exact driver,
run, scenario, profile, host, commit, and assertion joins block evidence
substitution; declared artifact paths are confined below the evidence root and
verified by byte count and SHA-256; the complete output manifest is verified
immediately after writing. Secret-like inline profile/observation data is
rejected. SHA-256 detects corruption and incompleteness under the trusted
runner boundary; it is not claimed as protection against a malicious runner.

UAT-CELLD-001 is a complete unattended compatibility gate. It runs the full
unit and VM-backed end-to-end regression with `AGENTIC_CELLD_ENABLED=false`
and points the Celld endpoint at a loopback TCP contact recorder. The E2E lane
uses a unique disposable VM, requests cleanup on every exit, and verifies both
its libvirt domain and storage directory are absent afterward. Any connection
attempt, regression failure, or cleanup failure fails the scenario. This makes
the automated target take several minutes on a warm host and longer on a cold
build.

```sh
node scripts/run-celld-uat.mjs --list
node scripts/run-celld-uat.mjs --trigger automated
node scripts/run-celld-uat.mjs --id UAT-CELLD-002
node scripts/run-celld-uat.mjs --id UAT-CELLD-003 \
  --run-id titan-123 --live-profile /protected/celld-live-profile.json
node --test tests/celld/uat/*.test.mjs
```

The public make targets preserve that boundary:

```sh
make test-celld-uat-automated
make test-celld-soak CELLD_SOAK_INPUT=/absolute/path/to/soak-input.json
make test-celld-human-uat CELLD_HUMAN_UAT_INPUT=/absolute/path/to/human-input.json
```

Heavy automated qualification is dispatched through
`.gitea/workflows/celld-qualification.yml`. That workflow is manual and pinned
to the `titan` runner (not the shared `rust` label). It serializes with the
existing VM E2E lane, verifies the exact commit and Titan host contract, and
requires at least 400 GiB free on `/build`, 100 GiB free on `/`, 32 GiB of
available memory, KVM/libvirt, Docker, a clean checkout, and a manifest-matched
base image before creating disposable resources. The workflow caps Cargo at
eight jobs with debug symbols and incrementality disabled, uses a job-scoped
target directory and quota-backed agentshare, previews every stale-VM reap,
and uploads the preflight plus UAT evidence even on failure.

The narrower `.gitea/workflows/celld-storage-qualification.yml` runs the full
10,000 create plus 10,000 overwrite S3-v1 gate against the pinned two-gateway
SeaweedFS topology. It is a construction slice, not a complete UAT-010 verdict:
it requires the storage and deterministic assertions to pass while preserving
the #765 network-isolation assertion as `NOT_RUN`. Raw per-round and aggregate
evidence is retained for 90 days. Neither workflow redirects existing local
storage, mounts, disks, volumes, workspaces, or management state.

Ordinary push and pull-request CI also has a bounded `celld-deterministic` job
on Titan. It runs the catalog/runner contracts plus `make test-celld` with at
most eight Cargo workers, gates build and image publication, and removes its
job-scoped compiler target afterward. It does not create a fleet or claim live
qualification.

The Titan workflow runs only scenarios 001-015. It executes the complete
disabled-path regression, static contract gate, and the shared deterministic
qualification support for every live scenario. `summary.json` reports both
the scenario verdicts and `supporting_checks` counts. The workflow
intentionally exits nonzero while any selected live scenario remains
`NOT_RUN`; moving execution off the workstation does not convert missing
drivers or credentials into evidence.
UAT 003-006, UAT 010 storage and network isolation, and UAT 012 signed
authentication have installed live drivers. A
registered-but-missing or disabled driver for the remaining live scenarios is
still a typed pre-mutation `NOT_RUN`; a profile cannot substitute a different
executable or self-declare an assertion verdict. UAT 013 remains intentionally
withheld behind the separate #766 authorization boundary.
The real 24-hour soak and representative-user session remain operator actions
through `make test-celld-soak` and `make test-celld-human-uat`.

Copy the templates under `operator/` outside the repository result directory,
replace every illustrative value, set `template_only` to `false`, and record
each artifact path relative to the input file. Operator evaluation refuses a
campaign shorter than 24 actual hours, missing personas or workflows,
placeholder or mismatched artifact hashes, secret-like input, and restricted
artifacts. It derives the verdict from raw measurements and observations; the
input cannot declare assertion PASS/FAIL values itself.

Results are written under the gitignored `tests/celld/uat/results/<run-id>/`
directory as `summary.json`, `evidence.jsonl`, `junit.xml`, `report.md`, and
`manifest.sha256`. Commands run without a shell. Their environment is not
captured, and stdout, stderr, and argv are redacted before evidence is hashed
or persisted. Bounded output heads and tails retain both startup context and
terminal driver summaries without storing unbounded logs.

Every summary labels its selection as `complete-uat-003-015` or
`partial-selection`. A partial run can prove its selected assertions but cannot
be presented as complete live qualification.

Exit codes are stable: `0` all selected scenarios passed, `1` an acceptance
assertion failed, `2` a prerequisite was unavailable or a scenario is
`NOT_RUN`, `3` catalog/evidence/runner validation failed, and `4` cleanup
failed. `NOT_RUN` is never a pass, and orchestration, Worker, fleet, security,
and operations verdicts remain independent. The soak evidence also records
separate orchestration, Worker, and fleet qualification decisions.

# Celld structured UAT

`scenarios.json` is the versioned acceptance catalog for issues #747-#754.
Scenarios 001-015 have the `automated` trigger. The actual 24-hour campaign
(016) and representative-user session (017) are explicitly operator-triggered.
The dependency-free runner executes only hardcoded deterministic executor IDs;
the catalog cannot inject shell commands. An automated live scenario remains
`NOT_RUN` until its disposable environment and driver supply the required
evidence. A partial unit result can pass an assertion while its scenario
remains `NOT_RUN`.

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

The Titan workflow runs only scenarios 001-015. It intentionally exits nonzero
while any selected live scenario remains `NOT_RUN`; moving execution off the
workstation does not convert missing drivers or credentials into evidence.
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

Exit codes are stable: `0` all selected scenarios passed, `1` an acceptance
assertion failed, `2` a prerequisite was unavailable or a scenario is
`NOT_RUN`, `3` catalog/evidence/runner validation failed, and `4` cleanup
failed. `NOT_RUN` is never a pass, and orchestration, Worker, fleet, security,
and operations verdicts remain independent. The soak evidence also records
separate orchestration, Worker, and fleet qualification decisions.

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
repository regression with `AGENTIC_CELLD_ENABLED=false` and points the Celld
endpoint at a loopback TCP contact recorder. Any connection attempt or any
regression failure fails the scenario. This makes the automated target take
several minutes on a cold build.

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

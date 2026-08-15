# Celld structured UAT

`scenarios.json` is the versioned acceptance catalog for issues #747-#754.
The dependency-free runner executes only hardcoded deterministic executor IDs;
the catalog cannot inject shell commands. Live scenarios remain `NOT_RUN` until
a future live harness supplies their recorded prerequisites. A partial unit
result can pass an assertion while its scenario remains `NOT_RUN`.

```sh
node scripts/run-celld-uat.mjs --list
node scripts/run-celld-uat.mjs --tag deterministic
node scripts/run-celld-uat.mjs --id UAT-CELLD-002
node --test tests/celld/uat/runner.test.mjs
```

Results are written under the gitignored `tests/celld/uat/results/<run-id>/`
directory as `summary.json`, `evidence.jsonl`, `junit.xml`, `report.md`, and
`manifest.sha256`. Commands run without a shell. Their environment is not
captured, and stdout, stderr, and argv are redacted before evidence is hashed
or persisted.

Exit codes are stable: `0` all selected scenarios passed, `1` an acceptance
assertion failed, `2` a prerequisite was unavailable or a scenario is
`NOT_RUN`, `3` catalog/evidence/runner validation failed, and `4` cleanup
failed. `NOT_RUN` is never a pass, and orchestration, Worker, fleet, security,
and operations verdicts remain independent.

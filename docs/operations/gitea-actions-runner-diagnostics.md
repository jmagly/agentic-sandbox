# Gitea Actions Runner Diagnostics

Use metadata-only inventory for routine runner diagnosis. A runner
registration token is a reusable credential in Gitea; retrieving it is a
provisioning operation, not a read-only health check.

## Safe repository inventory

Run the guarded helper with an existing `tea` login:

```bash
scripts/audit-gitea-runner-inventory.sh \
  --login local-gitea-admin \
  --owner roctinam \
  --repo agentic-sandbox
```

The helper calls only the repository runner-list endpoint and emits this
allowlisted metadata:

- runner ID and name;
- online, busy, and disabled state;
- ephemeral state;
- assigned labels.

It does not emit credentials, runner authentication material, environment
variables, or runner host details. Compare the returned IDs and names with the
approved runner inventory and with action-job history when investigating an
exposure window.

## Prohibited diagnostic path

Do not call any `actions/runners/registration-token` endpoint while checking
runner status. Those routes return a live reusable credential. The repository
runner-policy lint rejects that endpoint in workflows, scripts, and the
Makefile.

Never paste a token response, raw HTTP transcript, shell trace, or environment
dump into an issue or build log. Stop and treat the credential as exposed if a
diagnostic tool renders it.

## Rotation after exposure

An authenticated repository administrator rotates the credential from:

1. repository **Settings**;
2. **Actions** → **Runners**;
3. **Reset registration token**.

Gitea's reset action creates a replacement and deactivates the prior
repository-scoped registration tokens atomically. Do not copy or reveal the
replacement. Record only the administrator, timestamp, repository scope, and
successful reset notification as evidence.

After the reset, rerun the metadata-only inventory helper and verify that every
repository-scoped runner is expected. Existing registered runners keep their
own runner authentication and do not need the replacement registration token.

## Same-ref concurrency qualification

Gitea 1.25 could cancel an active same-ref run even when a workflow declared
`cancel-in-progress: false`. This installation relies on Gitea 1.27's
run-attempt concurrency implementation. Requalify that behavior after a Gitea
server migration or Actions scheduler change with the bounded
`.gitea/workflows/same-ref-concurrency-qualification.yml` harness.

Dispatch two `outcome=success` runs on the same ref within the first run's
bounded hold. Retain run, job, and task timestamps and verify that the second
run stays blocked until the first holder and its cleanup step complete. Neither
run may be cancelled. Then dispatch `outcome=failure` and verify its
`if: always()` cleanup step and evidence artifact complete before the expected
job failure.

The harness performs no release, VM, container, or provider mutation. It owns
only `/tmp/agentic-sandbox-concurrency-<run-id>` and removes that exact path.
Do not use release, E2E, or live qualification workflows to test scheduler
concurrency.

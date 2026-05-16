# [HIGH] HEALTH_TOKEN_PLACEHOLDER literal shipped in agentic-dev profile

**Labels**: `priority: high`, `area: security`, `area: bootstrap`, `type: incident`

## Summary

The `agentic-dev` cloud-init profile at `images/qemu/cloud-init/ubuntu.sh:238-242` writes `/etc/agentic-sandbox/health-token` containing the literal string `HEALTH_TOKEN_PLACEHOLDER` instead of substituting `$health_token`. The `basic` profile correctly substitutes. Inspect line ~712 for the same pattern in the second `write_files` block.

## Impact

For any VM provisioned with the agentic-dev profile, the health endpoint (rate-limited but otherwise the only path to the in-VM health server, typically port 8118) accepts the known string `HEALTH_TOKEN_PLACEHOLDER` as a valid bearer token. Any reachable client can:

```bash
curl -H "Authorization: Bearer HEALTH_TOKEN_PLACEHOLDER" http://<vm-ip>:8118/healthz
```

The agentic-dev profile is the **recommended** profile per CLAUDE.md, so this is the default exposure.

## Remediation

1. Replace the placeholder with `$health_token` in both profile templates in `cloud-init/ubuntu.sh`.
2. Add a CI assertion in `scripts/lint-cloud-init.sh`: grep generated cloud-init for `PLACEHOLDER` strings (case-sensitive) and fail.
3. Extend `validate-vm.sh` (per the existing VM-validation policy): after provisioning, assert the health endpoint rejects requests with the literal `HEALTH_TOKEN_PLACEHOLDER` and accepts only the real token from `vm-info.json`.

## Acceptance

- `grep -rn 'HEALTH_TOKEN_PLACEHOLDER' images/qemu/` returns only the CI assertion in `scripts/lint-cloud-init.sh`.
- New VMs reject placeholder token at health endpoint.

## References

- OWASP API Security Top 10: API2 Broken Authentication
- Internal audit finding H4 (secure-bootstrap-reviewer)

# Code-to-doc audit: activity observability delivery

- Date: 2026-08-01 (America/New_York)
- Direction: code-to-docs
- Baseline: `82edabb459b40b1dee330fba6e1de6f124b3e5bb`
- Reviewed head: `3f76b325b0350f702290d6888fe6e26e0e9db435`
- Scope: Agentic Sandbox issues #710-#716 and merged PRs #720-#726
- Status: APPROVED

## Result

No actionable documentation drift remains in the reviewed scope. The merged
documentation matches the activity event contract, Linux and network
collectors, governance/integrity controls, correlated timeline and query
surfaces, native macOS collector posture, and reliability campaign.

The audit specifically confirmed that the documentation preserves the two
important evidence boundaries:

1. Native macOS Endpoint Security activation still requires full Xcode,
   Apple entitlement approval, signing/notarization, and host consent.
2. The short #715 contract result is not presented as seven-day evidence; the
   production runtime matrix, source/action latency, host I/O, and storage
   budget gate remain open.

## Documentation surfaces reviewed

- `docs/observability/README.md`
- `docs/observability/linux-activity-collector.md`
- `docs/observability/network-activity-collector.md`
- `docs/observability/macos-activity-collector.md`
- `docs/observability/activity-governance.md`
- `docs/observability/activity-timeline.md`
- `docs/observability/activity-reliability-validation.md`
- `docs/API.md`, `docs/ECOSYSTEM.md`, `CHANGELOG.md`, and documentation manifests
- `.aiwg/architecture/adr/ADR-034-loss-aware-activity-event-contract.md`
- `.aiwg/architecture/adr/ADR-035-activity-governance-and-integrity.md`

## Findings

None. No product-documentation edits were required.

## Validation

The audit compared merged changes and documentation references with `git log`,
`git diff`, and targeted `rg` searches. Repository implementation validation
for the delivered scope passed `make lint`, `make test-unit`, and
`make test-scripts`; PR #726 docsite-build run 3344 also succeeded.

Detailed working evidence is retained under `.aiwg/working/doc-sync/` and is
intentionally excluded from the tracked delivery.

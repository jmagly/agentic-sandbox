# Code-to-documentation audit: v2026.8.3 release

- Date: 2026-08-03 (America/New_York)
- Direction: code-to-docs
- Baseline: `v2026.7.20`
- Reviewed head: `9c087dab207001fb4c1ffb16ef97ff35a61c6f02`
- Scope: `management`, `agent-rs`, `cli`, `scripts`, `images/qemu`, `docs`, `.aiwg`
- Status: APPROVED

## Result

The committed implementation surfaces are covered by product documentation.
The activity-observability lane already carried a post-merge code-to-doc audit;
the later operational-evidence and scheduler work documents its evidence
format, retention, validation bounds, and open seven-day qualification gate.
The fleet-workload lane includes a versioned schema, fixtures, normative spec,
API projection, and restart-reconciliation semantics. Container transport and
identity changes are reflected in the runtime, security-status, credential,
and autostart documentation.

Release-facing drift was resolved by adding the `2026.8.3` changelog entry and
operator release notes. No other high-confidence documentation fixes were
required.

## Evidence boundaries retained

1. Native macOS Endpoint Security activation is not claimed without Apple
   entitlement approval, full Xcode linkage, signing, notarization, and host
   consent.
2. Deterministic activity campaigns are not presented as seven-day production
   qualification evidence.
3. Explicit Docker compatibility transports, networks, or secret-bearing
   environment remain Tier 0 choices.
4. The Apple developer package remains unsigned and evaluation-only.

## Files changed

- `CHANGELOG.md`
- `docs/releases/v2026.8.3.md`
- `.aiwg/.last-doc-sync`
- `.aiwg/reports/doc-sync-audit-2026-08-03.md`

## Validation

Release hard-stop syntax, pin, formatting, and crate tests are recorded by the
release flow after this audit.

## Human review

No unresolved documentation drift. Publication wording remains subject to the
configured release-note threat-assessment gate.

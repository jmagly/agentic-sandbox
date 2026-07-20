# Architecture Evolution Report — Issue 411

**Date:** 2026-07-18
**Status:** Accepted / Implementation In Progress
**Trigger:** Enterprise certificate-provider extensibility and private
distribution guidance on `agentic-sandbox#411`

## Outcome

The accepted architecture is a public, versioned CLI provider protocol with
private out-of-process adapters. The current local CA remains built in. The
existing remote mock evolves into the public conformance provider. Concrete
OpenBao, step-ca, and SPIRE adapters share one private first-party monorepo and
are delivered as independently signed enterprise artifacts.

The baseline invokes providers on demand over bounded stdin/stdout, so no
provider service must remain loaded. Optional local-socket `serve` mode is
reserved for high-throughput fleets. Provider packages include detailed man
pages and machine-readable help for safe human/agent discovery.

## Artifacts

- Impact and option analysis:
  `@.aiwg/reports/impact-assessment-issue-411.md`
- Decision:
  `@.aiwg/architecture/adr/ADR-031-enterprise-certificate-provider-boundary.md`
- Migration and rollback:
  `@.aiwg/deployment/migration-plan-enterprise-ca-providers.md`
- Public contract overview:
  `@docs/security/enterprise-ca-provider-contract.md`
- Updated architecture baseline:
  `@.aiwg/architecture/agent-transport-security-sad.md`
- Updated risks:
  `@.aiwg/risks/agent-transport-security-risks.md`

## Review Panel

No subagents were dispatched because the repository instructions do not
authorize delegation for this request. The following review lenses were
applied directly.

| Lens | Status | Findings |
| --- | --- | --- |
| Architecture | Conditional | Out-of-process protocol is preferred; schema and compatibility tests remain implementation work |
| Security | Conditional | Core must validate all responses; adapter credentials must remain out of protocol/env/CLI/logs |
| Testing | Conditional | Conformance, IPC fault, rotation, live-PTY, and fake-secret sentinel tests are explicit gates |
| Operations | Conditional | Additional supervised process is acceptable only with health, provenance, rollback, and compatibility evidence |

## Breaking Changes

None in the planning change. The proposed implementation is additive and keeps
the default local backend. Future protocol-backed `remote` activation changes
fleet deployment requirements but not agent SPIFFE identity semantics.

## Approval Record

The operator approved the provider boundary, Gitea `roctinam` as authoritative,
GitHub `jmaly` as a private read-only mirror, OpenBao as the first adapter, and
the signed enterprise bundle. Follow-up guidance selected:

- one first-party enterprise-provider monorepo initially;
- CLI-first provider control and issuance;
- optional persistent service mode rather than a mandatory service;
- detailed man pages and machine-readable help for agents and operators.

Phase E1 may implement the public protocol and conformance harness without
creating or accessing provider credentials.

Phase E1 is complete in the issue #411 working tree. Its evidence and remaining
fleet-hardening gaps are recorded in
`@.aiwg/testing/enterprise-ca-provider-conformance.md`. Phase E3 requires a
separate confirmed external action to create the approved private repositories.

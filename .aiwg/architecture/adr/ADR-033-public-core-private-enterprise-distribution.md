# ADR-033: Public Core and Private Enterprise Distribution

## Status

Accepted (2026-07-24; operator direction under issue #462)

## Context

Agentic Sandbox is a complete public AGPL-3.0-only runtime. Enterprise
customers may also need vendor-specific certificate providers, identity and
authorization integrations, audit sinks, policy packs, supported deployment
composition, and commercially licensed artifacts.

ADR-031 already places enterprise certificate-provider mechanics behind a
versioned public command protocol, but proposed a dedicated private Gitea
repository. The current product direction follows the public-core/private
composition model used by AIWG and the capability-boundary discipline reviewed
in Fortemi ADR-095. The private source owner is `jmagly`.

## Decision

### Public core remains complete

The public repository owns runtime behavior, API and protocol contracts,
security validation, mocks and credential-free conformance fixtures, public
packages, checksums, SBOMs, provenance, and truthful capability discovery.
Enterprise licensing cannot disable or conceal public functionality.

### One private enterprise monorepo initially

`jmagly/agentic-sandbox-enterprise` is the private enterprise source,
composition, planning, and issue repository. It owns:

- first-party enterprise provider adapters;
- private-infrastructure integration tests;
- enterprise distribution manifests and edition metadata;
- customer deployment policy and runbooks;
- proprietary entitlement implementation;
- enterprise compatibility and release evidence.

The previously proposed `roctinam/agentic-sandbox-enterprise-ca` repository is
superseded by an `adapters/ca` area in this monorepo.

Split a capability into another repository only when distinct licensing,
ownership/access control, release cadence, third-party source, or supply-chain
isolation makes that boundary real.

### External versioned contracts are preferred

Private integrations should be out-of-process executables or services
implementing public contracts. An in-process enterprise build is exceptional
and requires a public interface, threat model, equivalent conformance tests,
and a documented reason process isolation is impossible. Native dynamic
library loading is not the default extension boundary.

### Compose without rewriting public artifacts

An enterprise release pins an exact public tag, commit, and artifact digest.
It may add private binaries, packages, service definitions, configuration, and
documentation. It must not mutate a public artifact after signature,
notarization, checksum, or provenance generation.

For macOS, enterprise additions use a separate signed package or an enclosing
distribution manifest. They do not unpack and repack the notarized public
package under the same identity.

### License only private capabilities

The public core starts and operates without an enterprise entitlement.
Enterprise components validate licenses independently and fail only the
requested enterprise-only capability closed. License verification must support
a bounded offline period, never transmit customer workload content, and never
place license-signing keys in application or CI configuration.

### Release relationship

Enterprise versions use `<public-calver>+ee.<revision>`, never lead the public
core, and record:

- public tag, commit, and artifact digests;
- supported public protocol major versions;
- private component commits and digests;
- SBOM, provenance, conformance, and integration evidence;
- license-policy version and bounded operator approval identifier.

Any required constituent or gate that silently skips makes the release
incomplete.

## Consequences

- Public users retain a complete and inspectable runtime.
- Proprietary providers and credentials stay out of the public process and
  repository.
- One private monorepo avoids premature repository/release fragmentation.
- Enterprise release engineering must maintain a strict compatibility matrix.
- Legal license text and live signing/license-issuance ceremonies remain
  human-controlled gates.

## References

- @.aiwg/architecture/adr/ADR-031-enterprise-certificate-provider-boundary.md
- @.aiwg/architecture/adr/ADR-032-apple-silicon-host-and-docker-first.md
- @docs/security/enterprise-ca-provider-contract.md
- Private: `https://github.com/jmagly/agentic-sandbox-enterprise`
- Public tracking: #462

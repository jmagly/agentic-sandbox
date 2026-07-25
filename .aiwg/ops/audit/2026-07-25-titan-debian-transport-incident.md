# Titan Debian Transport Incident Audit

date: 2026-07-25
issue: agentic-sandbox#679
systems:
  - agentic-sandbox
  - titan
  - gitea-actions

## Scope

Diagnose and repair verified Debian package-index retrieval for Titan CI
without exposing credentials, disabling TLS or apt signatures, installing host
software, rebooting, or disrupting active GPU/VM workloads.

## Initial State

- Public default branch: `f976ac75edd642df54a2ebdf8845efbc8b1929fe`.
- Failed docs job #98737 fetched 51-byte `InRelease` responses over plain HTTP.
- Failed Docker job #97760 reported TLS verification failures from pinned
  trixie build stages.
- Direct `ssh titan` could not use interactive hardware-backed identities.
- The existing passwordless automation profile `titan-agent` provided bounded
  read-only access.

## Commands and Evidence

- Inspected Gitea runs #2575, #2581, and #2597 and their job-log tails.
- Ran the AIWG address-issues threat preflight: `safe`, score 0.
- Dispatched temporary diagnostic run #2602 from
  `diag/679-titan-network` at `ee0776b5f1587bd9e2b3489e67a56dd6005adf9a`;
  direct host access later superseded it.
- Read-only Titan facts:
  - NTP synchronized;
  - Docker 29.1.3 / overlay2;
  - 12 running containers and 6 active GPU compute consumers;
  - host, Docker service, and both runner services had no proxy variables;
  - Docker daemon JSON contained only runtime/default-runtime configuration.
- Host transport probes:
  - plain HTTP: false `200`, 51 bytes, SHA-256
    `000bf7e61bfdc1f095ac3de7aabe694d0507a668292fb7d5c73c6e04dc11569c`;
  - HTTPS: `200`, 151075-byte signed index, TLS verification result 0.
- Disposable pinned-image proof found:
  - slim images need a CA bootstrap before switching apt to HTTPS;
  - copied CA-path symlinks are not self-contained across stages;
  - an explicit CA bundle with peer/hostname verification allows trixie index
    and package retrieval;
  - installing `ca-certificates` then refreshes the native trust store.
- Final disposable builds on physical Titan:
  - amd64 pinned Rust builder: pass;
  - amd64 bookworm runtime: pass;
  - amd64 trixie runtime: pass;
  - amd64 pinned Node docsite: pass;
  - arm64 bookworm runtime under QEMU: pass (53 seconds);
  - arm64 trixie runtime under QEMU: pass (56 seconds).

No proxy values, tokens, private keys, workload identities, or environment
dumps were printed or retained.

## Files Modified

- `.gitea/workflows/docsite-deploy.yml`
- `Dockerfile.dev`
- `Makefile`
- `ci/digests.txt`
- `deploy/docker/Dockerfile.agent-python`
- `deploy/docker/Dockerfile.agent-rust`
- `deploy/docker/Dockerfile.management`
- `images/container/Dockerfile.base`
- `scripts/prepare-debian-apt-https.sh` (new)
- `tests/container/test-debian-apt-https.sh` (new)
- `.aiwg/security/working/ci-workflow-audit.md` (new)
- `.aiwg/ops/audit/2026-07-25-titan-debian-transport-incident.md` (new)

## Temporary Artifacts

- Local proof Dockerfiles under `/tmp/agentic-sandbox-679-*`.
- Titan proof directory
  `/tmp/agentic-sandbox-679-proof-ee0776b`.
- Diagnostic branch `diag/679-titan-network`.

These contain no credentials. The proof directory and local temporary files
are removed after validation; the diagnostic commit remains recoverable by its
recorded SHA until branch cleanup.

## Assessment

The root cause is an outbound policy that returns a successful 51-byte response
for Debian plain HTTP, combined with pinned slim images that lack a usable
pre-bootstrap CA path. The repository fix makes Debian source transport
explicitly HTTPS, imports only a public CA trust store from an existing
digest-pinned stage, explicitly enables peer/hostname verification against the
bundle, and retains apt's signed-index verification.

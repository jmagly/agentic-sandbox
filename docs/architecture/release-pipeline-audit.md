# Release Pipeline Audit — 2026-05-19

**Trigger:** Cutting `v2026.5.1` revealed the release workflow runs in ~3 seconds and produces no binaries. Root cause: the release pipeline never had an artifact-build stage. This document inventories what exists, what's missing, and what needs to land before the next release.

**Scope:** All 8 workflows under `.gitea/workflows/`, plus the missing pieces between "tag pushed" and "user can install the release."

## 1. Workflow inventory

| Workflow | Triggers | What it does | What it does **not** do |
|---|---|---|---|
| `ci.yaml` | `push` to `main`/`develop`, `pull_request` | lint, test, build binaries, build + push `:latest` and `:<sha>` container images, run e2e | **Does not trigger on tag push.** Produces no `:v<version>`-tagged artifacts. Doesn't gate the release workflow. |
| `conformance.yml` | `push`/`PR` to `main`/`develop`, manual | Runs conformance harness | Doesn't trigger on tag; doesn't gate releases |
| `executor-build.yml` | `push`/`PR` to `main`/`develop` (path-filtered to executor crate), manual | `cargo check` + `cargo test --no-run` for `agentic-sandbox-executor` | Doesn't publish anything; partially duplicates ci.yaml's build job |
| `gitea-release.yaml` | `push` tags `v*` | Verifies Cargo versions match tag, pulls release notes from CHANGELOG, POSTs a Gitea release record | **Builds nothing. Attaches nothing. Doesn't wait for CI to pass on the tag commit.** |
| `schema-lint.yml` | `push`/`PR` to `main` (path-filtered to contracts) | Lints OpenAPI / contract schemas | n/a — single-purpose lint |
| `supply-chain-lint.yml` | `push`/`PR` to `main` (path-filtered to CI + Dockerfiles) | Enforces digest/SHA pinning | n/a — single-purpose lint |
| `docsite-build.yml` | `workflow_dispatch` only (push triggers commented out) | Builds the documentation site | Doesn't auto-build on docs changes; doesn't auto-build on release |
| `docsite-deploy.yml` | `workflow_dispatch` only (`tags: ['v*']` trigger commented out) | Builds + deploys docs site | **Tag-push deploy is wired but disabled.** Docs don't refresh on release. |

## 2. What happens today when a `v*` tag is pushed

```
git push origin v2026.5.1
        │
        ├─► gitea-release.yaml (3s)
        │     ✓ verify Cargo.toml versions
        │     ✓ extract CHANGELOG section
        │     ✓ POST /releases (creates the release page)
        │     ✗ NO artifact build
        │     ✗ NO check that CI passed on the tag commit
        │
        ├─► ci.yaml — DOES NOT RUN (no tag trigger)
        ├─► conformance.yml — DOES NOT RUN
        ├─► executor-build.yml — DOES NOT RUN (path-filtered, no executor change)
        ├─► docsite-deploy.yml — DOES NOT RUN (trigger commented out)
        └─► schema-lint.yml, supply-chain-lint.yml — DOES NOT RUN
```

Net effect: a release "happens" in 3 seconds, with no fresh build, no version-stamped containers, no binaries, no checksums, no provenance, no doc-site update, no GitHub release.

## 3. What the registry actually contains at release time

The internal registry at `git.integrolabs.net/roctinam/agentic-sandbox/*` carries:

- `agent:base`, `agent:dev`, `agent:latest`, `claude:latest`, `codex:latest`, `opencode:latest`, `mgmt:latest`, `agent-client:latest`
- `<image>:<git-sha>` for every commit pushed to `main`
- **No `<image>:v2026.5.1` (or any version-tagged release image)** — those tags are never created by any workflow

Consumers pinning to `:latest` get drift; consumers pinning to `:<sha>` get opaque hashes that don't correspond to any release. There is no way to pull "the 2026.5.1 release image."

## 4. Gap matrix vs filed issues

| Gap | Severity | Existing issue | New issue needed |
|---|---|---|---|
| No pre-release validation gate (CI-green, version-match, CHANGELOG presence) | P0 | [#295](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/295) | — |
| No version-tagged container images on tag push | P1 | partially [#299](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/299) (multi-registry — assumes versioned tags exist) | **new** — internal registry needs `:v<version>` tags first |
| No release binary tarballs + SHA256SUMS | P1 | [#297](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/297) | — |
| No cargo publish | P1 | [#296](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/296) | — |
| No SBOM / signatures | P2 | [#300](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/300) | — |
| No automated version bumping | P1 | [#301](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/301) | — |
| `ci.yaml` doesn't run on tag pushes | P1 | — | **new** — required so release builds produce stamped artifacts and the pre-release gate has signal |
| No GitHub release sync (tag pushed but no GitHub Release page entry) | P2 | — | **new** |
| `docsite-deploy.yml` tag trigger is commented out | P2 | — | **new** |
| `executor-build.yml` duplicates ci.yaml's build coverage | P3 | — | **new** (consolidation) |

## 5. Proposed release pipeline

```
                   ┌────────────────────────────────────────────────┐
                   │ Operator: aiwg/bump-version → CHANGELOG → tag  │
                   └─────────────────────┬──────────────────────────┘
                                         │
                              git push origin vX.Y.Z
                                         │
        ┌────────────────────────────────┴────────────────────────────────┐
        │                                                                  │
        ▼                                                                  ▼
┌──────────────────┐                                          ┌──────────────────────┐
│ ci.yaml (tag)    │                                          │ pre-release-gate     │
│  - lint+test     │  needs CI green ────────────────────►   │  - CI green on SHA   │
│  - build matrix  │                                          │  - version match     │
│  - container     │                                          │  - CHANGELOG match   │
│    :latest+:vX   │                                          │  - all conformance ✓ │
└────────┬─────────┘                                          └──────────┬───────────┘
         │                                                                │
         └──────────────────────► artifacts ◄──────────────────────────────┘
                                       │
        ┌──────────────┬───────────────┼──────────────────┬────────────────┐
        ▼              ▼               ▼                  ▼                ▼
  release-binaries  cargo-publish  cosign-sign       sbom-syft       docsite-deploy
  (tarballs+sums)   (3 crates)     (containers)      (CycloneDX)     (live docs)
        │              │               │                  │                │
        └──────────────┴───────────────┼──────────────────┴────────────────┘
                                       ▼
                          gitea-release (create + attach)
                                       │
                                       ▼
                          github-release-sync (mirror)
```

Each box is a workflow or job; arrows indicate dependency. The key change from today: every artifact-producing step is **gated by the pre-release validation step**, which itself depends on CI passing on the *tag* commit (not a prior branch push).

## 6. Phased remediation plan

### Phase 0 (this audit) — already done
- Documented gap, surfaced honestly in release notes (v2026.5.1 marked source-only).

### Phase 1 — pre-release safety net (P0)
Land [#295](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/295) (pre-release validation gate) + **new** "ci.yaml on tags" before any next release tag.

After this: a `v*` tag push runs CI fresh, and `gitea-release.yaml` blocks until CI is green on the tag commit. No more release-page entries for un-built code.

### Phase 2 — version-stamped artifacts (P1)
Land [#297](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/297) (binary tarballs + sums) and **new** "release-tagged container images on internal registry" + [#301](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/301) (version bumping tooling).

After this: each release has installable binaries with checksums, and the internal registry carries `:v<version>` tags. Users can pull and verify a specific release.

### Phase 3 — supply chain + multi-target (P1/P2)
Land [#296](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/296) (cargo publish), [#299](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/299) (multi-registry push), [#300](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/300) (sign + SBOM).

After this: crates.io install path, multi-registry container availability, end-to-end provenance.

### Phase 4 — automation polish (P2)
Land **new** GitHub release sync and **new** docsite-deploy on tag. Drop or consolidate `executor-build.yml`.

After this: one tag push = artifacts on Gitea + artifacts on GitHub + live docs site + working multi-registry pulls.

## 7. New issues to file

The audit surfaces four gaps not in any current issue:

1. **ci.yaml runs on tag pushes** — required so build/test/docker jobs produce artifacts for the tag commit
2. **Internal registry `:v<version>` container tags** — required by #299 (multi-registry push) and is a precondition for proper release pulls
3. **GitHub release sync** — GitHub mirror has the tag but no Releases page entry
4. **docsite-deploy.yml tag trigger** — re-enable so the doc site refreshes per release
5. **Consolidate `executor-build.yml`** — fold into `ci.yaml` to remove duplicate Cargo work

(Filed under issues #304–#308.)

## 8. Acceptance for a "fixed" release pipeline

After Phases 1–3 land, the next release MUST:

- [ ] CI runs and passes on the tag commit before the release record is created
- [ ] Release page has binary tarballs for x86_64-glibc, x86_64-musl, aarch64
- [ ] Release page has SHA256SUMS file alongside tarballs
- [ ] Internal registry has `:v<version>` tags for `mgmt`, `agent`, `agent-client`, `claude`, `codex`, `opencode`
- [ ] All three Rust crates published to crates.io
- [ ] All container images signed with cosign; SBOM attached via attestation
- [ ] GitHub mirror has a corresponding Releases page entry
- [ ] Docsite at `<docs-host>` reflects the release content

Anything short of that bar means we're shipping another source-only release and should mark it accordingly in the release notes.

# Release Pipeline Audit — 2026-05-19

**Trigger:** Cutting `v2026.5.1` revealed the release workflow runs in ~3 seconds and produces no binaries. Root cause: the release pipeline never had an artifact-build stage. This document inventories what exists, what's missing, and what needs to land before the next release.

**Scope:** All 8 workflows under `.gitea/workflows/`, plus the missing pieces between "tag pushed" and "user can install the release."

## 1. Workflow inventory

| Workflow | Triggers | What it does | What it does **not** do |
|---|---|---|---|
| `ci.yaml` | `push` to `main`/`develop`, `pull_request` | lint, test, build binaries, build + push `:latest` and `:<sha>` container images, run e2e | **Does not trigger on tag push.** Produces no `:v<version>`-tagged artifacts. Doesn't gate the release workflow. |
| `conformance.yml` | `push`/`PR` to `main`/`develop`, manual | Runs conformance harness | Doesn't trigger on tag; doesn't gate releases |
| `executor-build.yml` | `push`/`PR` to `main`/`develop` (path-filtered to executor crate), manual | `cargo check` + `cargo test --no-run` for `agentic-sandbox-executor` | Doesn't publish anything; partially duplicates ci.yaml's build job |
| `gitea-release.yaml` | ~~`push` tags `v*`~~ — **removed in Phase 2**; consolidated into `ci.yaml` `release-attach` job | ~~Verified Cargo versions match tag, pulled release notes from CHANGELOG, POSTed a release record~~ | n/a (deleted) |
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

The internal registry at `registry.example.invalid/agentic-sandbox/*` carries:

- `agent:base`, `agent:dev`, `agent:latest`, `claude:latest`, `codex:latest`, `opencode:latest`, `automation-control:latest`, `agentic-mgmt:latest`, `agent-client:latest`
- `<image>:<git-sha>` for every commit pushed to `main`
- **No `<image>:v2026.5.1` (or any version-tagged release image)** — those tags are never created by any workflow

Consumers pinning to `:latest` get drift; consumers pinning to `:<sha>` get opaque hashes that don't correspond to any release. There is no way to pull "the 2026.5.1 release image."

## 4. Gap matrix vs filed issues

| Gap | Severity | Existing issue | New issue needed |
|---|---|---|---|
| No pre-release validation gate (CI-green, version-match, CHANGELOG presence) | P0 | [#295](https://github.com/jmagly/agentic-sandbox/issues/295) | — |
| No version-tagged container images on tag push | P1 | partially [#299](https://github.com/jmagly/agentic-sandbox/issues/299) (multi-registry — assumes versioned tags exist) | **new** — internal registry needs `:v<version>` tags first |
| No release binary tarballs + SHA256SUMS | P1 | [#297](https://github.com/jmagly/agentic-sandbox/issues/297) | — |
| No cargo publish | P1 | [#296](https://github.com/jmagly/agentic-sandbox/issues/296) | — |
| No SBOM / signatures | P2 | [#300](https://github.com/jmagly/agentic-sandbox/issues/300) | — |
| No automated version bumping | P1 | [#301](https://github.com/jmagly/agentic-sandbox/issues/301) | — |
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
Land [#295](https://github.com/jmagly/agentic-sandbox/issues/295) (pre-release validation gate) + **new** "ci.yaml on tags" before any next release tag.

After this: a `v*` tag push runs CI fresh, and `gitea-release.yaml` blocks until CI is green on the tag commit. No more release-page entries for un-built code.

### Phase 2 — version-stamped artifacts (P1) — **landed 2026-05-19**

Implemented in commits `89440ba` (Phase 1: #295 + #304 + #305) and `a784283` (#301: version bump tooling) and this commit (#297: release binary tarballs + SHA256SUMS):

- `release-binaries` job (tag-only, matrix: `x86_64-unknown-linux-gnu` + `x86_64-unknown-linux-musl`) builds `agentic-mgmt`, `vm-event-bridge`, `agent-client`, `sandboxctl` and packages them into `agentic-sandbox-vX.Y.Z-<arch>-<libc>.tar.gz` with per-file `.sha256` sidecar. The `vm-event-bridge` binary ships with the x86_64 Linux GNU management artifact because it links to libvirt like `agentic-mgmt`.
- `release-linux-packages` job (tag-only, x86_64 Linux GNU) builds package-manager artifacts `agentic-sandbox_<version>-1_amd64.deb` and `agentic-sandbox-<version>-1.x86_64.rpm`, stages the versioned `agentic-sandbox-install.sh` installer, validates metadata/file lists, runs installer dry-runs, runs clean Debian/RPM-family install-uninstall smoke tests, and publishes package checksums alongside the tarballs.
- AppImage is not part of the initial Linux package matrix. Unlike HotM, `agentic-sandbox` is primarily a service/CLI/runtime bundle with systemd units, libvirt integration, environment templates, and host state directories; `.deb`/`.rpm` packages model those ownership and dependency contracts directly. Revisit AppImage only if a single-user desktop/operator wrapper becomes a first-class deliverable.
- `release-binaries-mutsu` is disabled while Darwin/macOS release artifacts are deferred. The mutsu SSH implementation remains in the workflow as implementation history, but it is not release-blocking and `MUTSU_SSH_KEY` is not required for current production tags. `agentic-mgmt`/Apple VM provider support remains tied to #438.
- `release-attach` job (tag-only, gates on `release-binaries` + `release-linux-packages` + `docker` + `integration`) downloads the matrix artifacts, generates a canonical `SHA256SUMS` file across tarballs, native packages, and installer script, creates the Gitea release, and attaches every release asset plus checksum sidecars.
- `gitea-release.yaml` deleted; its responsibility lives in `release-attach`.
- Windows is explicitly deferred from the current release matrix (#482). There is no supported Windows runtime/provider story for `agentic-mgmt` or `agent-client` yet; the likely first Windows package is a future `sandboxctl.exe` operator-client installer once a pinned Windows builder and smoke-test lane exist.

After this: each release has installable binaries with checksums, and the internal registry carries `:v<version>` tags. Users can pull and verify a specific release.

**Phase 2 status:**
- `aarch64-apple-darwin` — **deferred**; the previous mutsu SSH lane is disabled and is no longer part of the public release matrix. Re-enable only when macOS support is promoted again, with a clear product scope for host-direct CLI artifacts, `.dmg`, or Apple runtime/provider support.
- `aarch64-unknown-linux-gnu` — **landed (#311 resolved)**, with a caveat: ships `agent-client` + `sandboxctl` only. `agentic-mgmt` is excluded because it hard-links to the system libvirt C library and no aarch64-linux libvirt sysroot is available on mutsu. The aarch64-linux tarball includes a `MGMT_EXCLUDED.txt` note documenting this and pointing at the x86_64-linux-gnu archive for control-plane use.

Resolution path for #311 (committed):
- `reqwest` + `tokio-tungstenite` switched from `native-tls` to `rustls`/`rustls-tls-webpki-roots`.
- `josekit` (used by the executor for AgentCard JWS signing) pinned to vendored `openssl` since it hard-depends on openssl. The C openssl compiles from source as part of the build (~30s overhead per cold build).
- `cargo-zigbuild` does the cross-link with zig as the linker; cargo `net.git-fetch-with-cli = true` set on mutsu so cargo uses system git for fetches against `internal Git host` (libgit2 SSL handshake failed for that origin).

### Phase 3 — supply chain + multi-target (P1/P2) — **wired 2026-05-19**

Implemented (job surface in `ci.yaml`; gated on operator-provided secrets):

- [#296](https://github.com/jmagly/agentic-sandbox/issues/296) — `cargo-publish` job. `cargo publish --dry-run` then real publish in dep order. Skip-with-warning when `CARGO_REGISTRY_TOKEN` not set.
- [#299](https://github.com/jmagly/agentic-sandbox/issues/299) / [#478](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/478) — `multi-registry-push` job. Mirrors the release-tagged runtime image set (`agentic-mgmt`, `agent-client`, `agent`, `claude`, `codex`, `opencode`, `automation-control`) to public package names under `ghcr.io/<owner>/agentic-sandbox-*` and optional Quay equivalents. GHCR is release-blocking; after the authenticated push and digest capture, the workflow logs out of `ghcr.io` and smoke-pulls the management and agent-client images anonymously so the gate proves public user access, not only credentialed publication. Optional mirrors skip only when explicitly documented.
- [#300](https://github.com/jmagly/agentic-sandbox/issues/300) — `sign-and-sbom` job. GPG-signs tarballs (detached `.asc`), generates syft CycloneDX SBOMs per tarball and per GHCR release image, and cosign-signs internal + GHCR container images. Signing capabilities gate on their secrets; SBOM generation runs without signing keys.

After this: crates.io install path, multi-registry container availability, end-to-end provenance. Activation requires the operator to provision secrets per `docs/releases/runbook.md` § Required secrets.

### Phase 4 — automation polish (P2) — **wired 2026-05-19**

Implemented:

- [#306](https://github.com/jmagly/agentic-sandbox/issues/306) — `github-release-sync` job in `ci.yaml`. Idempotent `gh release create`/`edit` against `jmagly/agentic-sandbox` after Gitea release lands; mirrors notes + tarballs + checksums. Skip-with-warning when `GITHUB_MIRROR_TOKEN` not set.
- [#307](https://github.com/jmagly/agentic-sandbox/issues/307) — `docsite-deploy.yml` `push.tags: ['v*']` trigger re-enabled. Job now guards on the deploy-stack secrets and skips with warning when not configured.
- [#308](https://github.com/jmagly/agentic-sandbox/issues/308) — `executor-build.yml` deleted; `Makefile test-unit` updated to `cargo test --workspace` so executor-crate coverage flows through normal CI.

After this: one tag push = artifacts on Gitea + artifacts on GitHub + live docs site + signed/SBOM'd containers + crates.io publish, **once secrets are provisioned**.

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
- [ ] Darwin/macOS artifacts are explicitly documented as deferred and are not release-blocking
- [ ] Release page has native x86_64 Linux packages (`.deb` and `.rpm`)
- [ ] Release page has the HotM-style Linux installer script (`agentic-sandbox-install.sh`) and checksum coverage
- [ ] Release docs explicitly state Windows is deferred until #482, rather than implying Windows parity exists
- [ ] Release page has SHA256SUMS file alongside tarballs
- [ ] Internal registry has `:v<version>` tags for `agentic-mgmt`, `agent-client`, `agent`, `claude`, `codex`, `opencode`, `automation-control`
- [ ] All three Rust crates published to crates.io
- [ ] All container images signed with cosign; SBOM attached via attestation
- [ ] GitHub mirror has a corresponding Releases page entry
- [ ] Docsite at `<docs-host>` reflects the release content

Anything short of that bar means we're shipping another source-only release and should mark it accordingly in the release notes.

# Release Runbook

End-to-end procedure for cutting an `agentic-sandbox` release. Source of truth for the steps a human (or agent) takes between "merge to main" and "tag pushed."

This runbook is paired with the CI pipeline: most of the work is mechanical, automated, and gated. The runbook tells you which knob to turn and what to watch for; CI tells you whether it worked.

## When to release

- **Patch bump** (`2026.5.X+1`): bug fixes only, no behavior change for v1/v2 clients. Cut whenever there is at least one shippable fix.
- **Minor / month bump** (`2026.6.0`): feature work, new contract surface, contract-aligned improvements.
- **Major bump**: reserved for v3.0 (v1 removal) per ADR-018. Not a calendar-driven cut.

CalVer format: `YYYY.M.PATCH`, **no leading zeros** in any component (see `.claude/rules/versioning.md`).

## Pre-flight checklist

Before starting:

- [ ] Working tree clean on `main`
- [ ] `git pull --ff-only origin main` (sync local with origin)
- [ ] Last CI run on `main` was green (`gh run list --branch main --limit 1` or the Gitea Actions view)
- [ ] No open PRs you're waiting on
- [ ] You know the next version number

## Step 1 — Bump versions

```bash
scripts/bump-version.sh 2026.5.3
```

What this does:

- Validates the version format (CalVer, no leading zeros)
- Fails if working tree is dirty
- Fails if `CHANGELOG.md` already has a `## [<version>]` section (idempotency guard)
- Updates `management/Cargo.toml`, `agent-rs/Cargo.toml`, `cli/Cargo.toml`
- Updates the matching `Cargo.lock` entries
- Inserts a new `## [<version>] — <today>` section under `## [Unreleased]` in `CHANGELOG.md` with placeholder Added / Changed / Fixed / Documentation / Operator-notes headings
- Updates the `[Unreleased]` and inserts a new `[<version>]` compare-link in the CHANGELOG footer

Optional:

```bash
scripts/bump-version.sh 2026.5.3 --dry-run       # show the plan without writing
scripts/bump-version.sh 2026.5.3 --date 2026-06-01  # stamp a non-today date
```

If the script fails with "working tree is dirty," commit or stash your in-flight work first. Don't bypass the check — version bumps that mix with unrelated changes break the audit trail.

## Step 2 — Populate the CHANGELOG section

Open `CHANGELOG.md` and replace the placeholder body in the new `## [<version>]` section with the actual list of changes. Source the content from:

```bash
git log v<previous-version>..HEAD --pretty=format:'%h %s' --no-merges
```

Group commits by conventional-commit type:

- `feat:` → `### Added`
- `fix:` → `### Fixed`
- `security:` → `### Security`
- `docs:` → `### Documentation`
- `chore:` / `refactor:` → `### Changed` (when user-visible) or omit

If the release ships from source without artifacts (Phase 1/2 of `release-pipeline-audit.md` not yet met for some category), include the "Source-only release" notice quote-block at the top of the section. Reference: the v2026.5.1 and v2026.5.2 entries in `CHANGELOG.md`.

## Step 3 — Write the release announcement

Create `docs/releases/v<version>.md` from the template at the top of an existing announcement (`docs/releases/v2026.5.2.md` is the most recent reference). Must include:

- Header block: Released / Tag / Previous / Compare
- Source-only notice (if applicable)
- Highlights (3–7 bullet points)
- Upgrade matrix per audience
- Verification steps (commands the user can run to confirm the upgrade)

The announcement and the CHANGELOG section can repeat content; the CHANGELOG is the source of truth and the announcement is the welcoming surface.

## Step 4 — Commit

```bash
git add -A
git status --short   # review what's about to land
git commit -m "$(cat <<EOF
chore(release): bump to <version> + add CHANGELOG and announcement

<short summary>

Closes <relevant issue numbers if any>
EOF
)"
```

Push to main first so CI can run on the commit BEFORE the tag exists:

```bash
git push origin main
git push github main
```

Wait for CI on `main` to complete and turn green. The pre-release gate (`prerelease-gate` job in `ci.yaml`) is skipped on branch pushes, so this is a normal CI run. Watch for any test or lint regressions.

## Step 5 — Tag and push

```bash
git tag -a v<version> -m "$(cat <<EOF
v<version> — <one-line summary>

<3–5 bullet highlights>

Full notes: docs/releases/v<version>.md and CHANGELOG.md [<version>]
EOF
)"

git push origin v<version>
git push github v<version>
```

## Step 6 — Verify the release pipeline

Pushing the tag triggers two workflows (post Phase 1 of `release-pipeline-audit.md`):

1. **`ci.yaml`** runs in tag context. Watch for:
   - `prerelease-gate` job passes (verifies Cargo + CHANGELOG match the tag)
   - `docker` job tags images `:latest`, `:<sha>`, AND `:v<version>` on the internal registry
   - All other jobs green
2. **`gitea-release.yaml`** triggers via `workflow_run` after CI completes. Watch for:
   - Conclusion check (only fires if CI succeeded and ref starts with `v`)
   - Defense-in-depth version + CHANGELOG re-verification
   - Release record POSTed to Gitea

Check the registry for the new tag:

```bash
TOKEN=$(cat ~/.config/gitea/admin-token)
curl -s -H "Authorization: token ${TOKEN}" \
  "https://git.integrolabs.net/api/v1/packages/roctinam?type=container&q=mgmt&limit=10" \
  | jq -r '.[] | "\(.name):\(.version)"' | grep v<version>
```

Check the release page exists:

```bash
curl -s -H "Authorization: token ${TOKEN}" \
  "https://git.integrolabs.net/api/v1/repos/roctinam/agentic-sandbox/releases/tags/v<version>" \
  | jq '{tag: .tag_name, asset_count: (.assets | length)}'
```

## Step 7 — Smoke test (optional but recommended)

Pull the released container image and run a smoke check:

```bash
docker pull git.integrolabs.net/roctinam/agentic-sandbox/mgmt:v<version>
docker run --rm git.integrolabs.net/roctinam/agentic-sandbox/mgmt:v<version> --version
# Should print: 2026.5.3 (or whatever <version> is)
```

## Rollback procedure

If a release is cut with broken content (wrong version, missing CHANGELOG section, broken binary):

1. **Delete the Gitea release record** — keep the tag for history but unpublish the release page:
   ```bash
   curl -s -X DELETE -H "Authorization: token ${TOKEN}" \
     "https://git.integrolabs.net/api/v1/repos/roctinam/agentic-sandbox/releases/<release-id>"
   ```
2. **Do NOT delete the tag** unless it was never published anywhere (rare). Tag deletion breaks any reference to it.
3. **Cut a new patch release** (`X.Y.Z+1`) with the fix.
4. **Update the broken release's CHANGELOG section** to add a "Superseded by [X.Y.Z+1]" notice at the top.
5. If artifacts were pushed to the registry under the broken `:v<version>` tag, they remain — there's no clean way to delete a container tag without affecting consumers. The patch release shipping `:v<X.Y.Z+1>` is the canonical pointer.

## CI runner assignments

| Runner | Labels | What lands here |
|---|---|---|
| **`titan`** (large build server) | `titan, rust, gpu, matric-builder, ubuntu-latest, node-20, deploy` | test, build, docker, e2e, conformance, release-binaries (x86_64), cargo-publish, multi-registry-push, sign-and-sbom |
| **`teroknor`** (small DMZ / network host) | `teroknor, docker, ubuntu-22.04, ubuntu-24.04, ubuntu-latest, node-20` | prerelease-gate, lint, security scan, supply-chain-lint, schema-lint, release-binaries-mutsu (SSH out), release-attach, github-release-sync |
| ~~`grissom`~~ | `self-hosted, ubuntu-*` | **Never** — workstation, NOT a build server. No CI job in this repo targets `runs-on: self-hosted`. |

Workflows reference runners by **specific label** (`runs-on: titan` or `runs-on: teroknor`), never `self-hosted`. While #367 remains open, treat `titan` as a runner label contract rather than proof of one physical host: release E2E logs include a substrate preflight, VM-backed E2E is serialized with the `agentic-sandbox-vm-e2e` concurrency group, and x86 release binary builds run one matrix entry at a time with `CARGO_BUILD_JOBS=8` to reduce contention on the shared titan lane.

### Docker lane runner exec recovery (#335)

A Docker Build & Publish failure that reports `fork/exec /usr/bin/bash: operation not permitted` before project commands run is a host runner exec failure, not a repository build failure. The workflow cannot self-retry that condition once the runner cannot start the shell for a step.

Recovery path:

1. Check whether the same commit already passed PR CI and whether another run on the same commit passes the Docker job.
2. Inspect the Docker lane preflight in successful starts for host identity, runner labels, `/usr/bin/bash` metadata, Docker version, and Cargo version.
3. Re-run `ci.yaml` with `workflow_dispatch` against the same ref after the runner service has recovered or been restarted by an operator.
4. Treat repeated bash exec failures on the same host as runner infrastructure work: remove the runner from the `titan` label pool or repair the act_runner service before using the result as release evidence.

## Required secrets

The Phase 2/3 release jobs in `ci.yaml` and `docsite-deploy.yml` are wired but skip-with-warning when their secrets are absent. Provision these in **Repo Settings → Actions → Secrets** to activate each job:

| Secret(s) | Activates | Notes |
|---|---|---|
| `CARGO_REGISTRY_TOKEN` | `cargo-publish` job (#296) | crates.io API token; needs publish permission on all three crates |
| `GHCR_TOKEN` | `multi-registry-push` job (#299) — GHCR half | GitHub PAT with `write:packages`; pushes to `ghcr.io/jmagly/<image>:<tag>` |
| `QUAY_USERNAME`, `QUAY_PASSWORD` | `multi-registry-push` job (#299) — Quay half | Robot account credentials |
| `COSIGN_KEY`, `COSIGN_PASSWORD` | `sign-and-sbom` job (#300) — container signing | `cosign generate-key-pair` output |
| `GPG_PRIVATE_KEY`, `GPG_PASSPHRASE` | `sign-and-sbom` job (#300) — tarball signing | Armored private key; `gpg --export-secret-keys --armor <fpr>` |
| `GITHUB_MIRROR_TOKEN` | `github-release-sync` job (#306) | GitHub PAT with `repo` scope on `jmagly/agentic-sandbox` |
| `GT_ACCESS_TOKEN`, `DEPLOY_SSH_KEY`, `DEPLOY_HOST`, `DEPLOY_PORT`, `DEPLOY_USER`, `DEPLOY_PATH` | `docsite-deploy` (#307) | Tracked in issue [#194](https://git.integrolabs.net/roctinam/agentic-sandbox/issues/194) |
| `MUTSU_SSH_KEY` | `release-binaries-mutsu` (aarch64-apple-darwin + aarch64-unknown-linux-gnu) | PEM private key for `manitcor@10.0.42.41`. The `teroknor` runner SSHes to mutsu to run the build (per the fortemi/publish-sidecar.yml pattern — the native `runs-on: mutsu` path has a known reverse-proxy / gRPC fetch issue). |

Until any given set is provisioned, the corresponding job runs and emits `::warning::` log lines explaining what's missing — no failure, no broken release.

## What's still deferred

| Step | Status | Issue |
|---|---|---|
| aarch64 binary target | deferred — needs runner setup on mutsu | [`docs/architecture/aarch64-build-runner-plan.md`](../architecture/aarch64-build-runner-plan.md) |

Releases that ship without secrets configured must include the "Source-only release" notice in their CHANGELOG section and announcement.

## References

- `docs/architecture/release-pipeline-audit.md` — full audit of what CI does and doesn't do per release
- `.claude/rules/versioning.md` — CalVer format rules
- `.gitea/workflows/ci.yaml` — Phase 1 release-pipeline integration
- `.gitea/workflows/gitea-release.yaml` — workflow_run-triggered release creation
- `scripts/bump-version.sh` — the version-bump script invoked in Step 1

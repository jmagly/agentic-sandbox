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
- Package install/upgrade commands when native packages or installer assets ship
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
   - Release record published

Check the registry for the new tag:

```bash
TOKEN=$(cat ~/.config/gitea/admin-token)
curl -s -H "Authorization: token ${TOKEN}" \
  "https://registry.example.invalid/api/v1/packages/agentic-sandbox?type=container&q=agentic-mgmt&limit=10" \
  | jq -r '.[] | "\(.name):\(.version)"' | grep v<version>
```

Check the public GHCR packages:

```bash
for image in \
  agentic-sandbox-mgmt \
  agentic-sandbox-agent-client \
  agentic-sandbox-agent \
  agentic-sandbox-claude \
  agentic-sandbox-codex \
  agentic-sandbox-opencode \
  agentic-sandbox-automation-control; do
  docker pull ghcr.io/<owner>/${image}:v<version>
done
```

This check must run without an active `ghcr.io` Docker login. GitHub Container
Registry packages are private on first publish unless their package visibility
is changed; anonymous `docker pull` is the release proof that the packages are
public for users.

Example compose service using the public management image:

```yaml
services:
  agentic-mgmt:
    image: ghcr.io/<owner>/agentic-sandbox-mgmt:v<version>
    command: ["agentic-mgmt"]
    restart: "no"
```

Check the release page exists:

```bash
curl -s -H "Authorization: token ${TOKEN}" \
  "https://api.github.com/repos/jmagly/agentic-sandbox/releases/tags/v<version>" \
  | jq '{tag: .tag_name, asset_count: (.assets | length)}'
```

Verify native Linux packages are attached:

```bash
curl -s -H "Authorization: token ${TOKEN}" \
  "https://api.github.com/repos/jmagly/agentic-sandbox/releases/tags/v<version>" \
  | jq -r '.assets[].name' \
  | grep -E 'agentic-sandbox_.*_amd64\.deb|agentic-sandbox-.*\.x86_64\.rpm|agentic-sandbox-install\.sh'
```

## Step 7 — Smoke test (optional but recommended)

Pull the released container image and run a smoke check:

```bash
docker pull registry.example.invalid/agentic-sandbox/agentic-mgmt:v<version>
docker run --rm --entrypoint /bin/sh registry.example.invalid/agentic-sandbox/agentic-mgmt:v<version> \
  -lc 'command -v agentic-mgmt >/dev/null && test -x "$(command -v agentic-mgmt)"'
docker pull ghcr.io/<owner>/agentic-sandbox-mgmt:v<version>
docker run --rm --entrypoint /bin/sh ghcr.io/<owner>/agentic-sandbox-mgmt:v<version> \
  -lc 'command -v agentic-mgmt >/dev/null && test -x "$(command -v agentic-mgmt)"'
```

Download and inspect the native Linux packages:

```bash
dpkg-deb --info agentic-sandbox_<version>-1_amd64.deb
dpkg-deb --contents agentic-sandbox_<version>-1_amd64.deb | grep /usr/bin/sandboxctl
dpkg-deb --contents agentic-sandbox_<version>-1_amd64.deb | grep /usr/bin/agentic-sandbox
mkdir -p /tmp/agentic-sandbox-rpmdb
rpm --dbpath /tmp/agentic-sandbox-rpmdb -qip agentic-sandbox-<version>-1.x86_64.rpm
rpm --dbpath /tmp/agentic-sandbox-rpmdb -qlp agentic-sandbox-<version>-1.x86_64.rpm | grep /usr/bin/sandboxctl
rpm --dbpath /tmp/agentic-sandbox-rpmdb -qlp agentic-sandbox-<version>-1.x86_64.rpm | grep /usr/bin/agentic-sandbox
sha256sum -c SHA256SUMS-linux-packages
```

One-line Linux installer:

```bash
curl -fsSL https://github.com/jmagly/agentic-sandbox/releases/download/v<version>/agentic-sandbox-install.sh \
  | bash -s -- --version v<version>
```

Installer dry-run checks against downloaded assets:

```bash
bash agentic-sandbox-install.sh --local-package agentic-sandbox_<version>-1_amd64.deb --dry-run
bash agentic-sandbox-install.sh --local-package agentic-sandbox-<version>-1.x86_64.rpm --dry-run
bash agentic-sandbox-install.sh --local-deb agentic-sandbox_<version>-1_amd64.deb --dry-run
bash agentic-sandbox-install.sh --local-rpm agentic-sandbox-<version>-1.x86_64.rpm --dry-run
```

Clean package install/uninstall smoke, matching CI:

```bash
PACKAGES_DIR=. tests/package/smoke-linux-packages.sh --required
```

Direct package install examples, if bypassing the installer:

```bash
sudo apt-get install ./agentic-sandbox_<version>-1_amd64.deb
sudo dnf install ./agentic-sandbox-<version>-1.x86_64.rpm
```

macOS Apple Silicon host-direct artifacts are deferred from the current public release matrix. Do not block a production tag on `aarch64-darwin` tarballs or `MUTSU_SSH_KEY` while this scope is deferred; macOS runtime/provider support remains tied to #438.

Windows is not part of the current release matrix. The deferred platform decision is tracked in #482: the likely first Windows deliverable is a `sandboxctl.exe` operator-client package, while `agent-client.exe`, `agentic-mgmt.exe`, and Windows VM/provider support require an explicit runtime/provider design before CI publishes installers.

## Rollback procedure

If a release is cut with broken content (wrong version, missing CHANGELOG section, broken binary):

1. **Delete the release record** — keep the tag for history but unpublish the release page:
   ```bash
   curl -s -X DELETE -H "Authorization: token ${TOKEN}" \
     "https://github.com/jmagly/agentic-sandbox/releases/<release-id>"
   ```
2. **Do NOT delete the tag** unless it was never published anywhere (rare). Tag deletion breaks any reference to it.
3. **Cut a new patch release** (`X.Y.Z+1`) with the fix.
4. **Update the broken release's CHANGELOG section** to add a "Superseded by [X.Y.Z+1]" notice at the top.
5. If artifacts were pushed to the registry under the broken `:v<version>` tag, they remain — there's no clean way to delete a container tag without affecting consumers. The patch release shipping `:v<X.Y.Z+1>` is the canonical pointer.

## CI runner assignments

| Runner | Labels | What lands here |
|---|---|---|
| **`titan`** (large build server) | `titan, rust, gpu, matric-builder, ubuntu-latest, node-20, deploy` | test, build, docker, e2e, conformance, release-binaries (x86_64), release-linux-packages, cargo-publish, multi-registry-push, sign-and-sbom |
| **`teroknor`** (small DMZ / network host) | `teroknor, docker, ubuntu-22.04, ubuntu-24.04, ubuntu-latest, node-20` | prerelease-gate, lint, security scan, supply-chain-lint, schema-lint, release-attach, github-release-sync |
| ~~`grissom`~~ | `self-hosted, ubuntu-*` | **Never** — workstation, NOT a build server. No CI job in this repo targets `runs-on: self-hosted`. |

Workflows reference runners by **specific label** (`runs-on: titan` or `runs-on: teroknor`), never `self-hosted`. The accepted #363/#367 runner posture treats `titan` as a runner label contract rather than proof of one physical host: release E2E logs include a substrate preflight, VM-backed E2E is serialized with the `agentic-sandbox-vm-e2e` concurrency group, and x86 release binary builds run one matrix entry at a time with `CARGO_BUILD_JOBS=8` to reduce contention on the shared titan lane.

The post-#312 E2E cooldown is complete as of the #316 follow-up: `ci.yaml` no longer keeps E2E tag-only. Branch and main pushes now exercise the VM-backed E2E gate before release tags depend on it, while release publication jobs still require successful tag-context E2E.

### Docker lane runner exec recovery (#335)

A Docker Build & Publish failure that reports `fork/exec /usr/bin/bash: operation not permitted` before project commands run is a host runner exec failure, not a repository build failure. The workflow cannot self-retry that condition once the runner cannot start the shell for a step.

Recovery path:

1. Check whether the same commit already passed PR CI and whether another run on the same commit passes the Docker job.
2. Inspect the Docker lane preflight in successful starts for host identity, runner labels, `/usr/bin/bash` metadata, Docker version, and Cargo version.
3. Re-run `ci.yaml` with `workflow_dispatch` against the same ref after the runner service has recovered or been restarted by an operator.
4. Treat repeated bash exec failures on the same host as runner infrastructure work: remove the runner from the `titan` label pool or repair the act_runner service before using the result as release evidence.

## Required secrets

The Phase 2/3 release jobs in `ci.yaml` and `docsite-deploy.yml` are wired to fail closed for required release surfaces and skip-with-warning only for optional surfaces. Provision these in **Repo Settings → Actions → Secrets** before cutting a production tag:

| Secret(s) | Activates | Notes |
|---|---|---|
| `CARGO_REGISTRY_TOKEN` | `cargo-publish` job (#296) | crates.io API token; needs publish permission on all three crates |
| `GHCR_TOKEN` | `multi-registry-push` job (#299/#478) — public GHCR packages | Required for production tag releases. GitHub PAT with `write:packages`; pushes `ghcr.io/${GHCR_OWNER:-jmagly}/agentic-sandbox-{mgmt,agent-client,agent,claude,codex,opencode,automation-control}:<tag>` |
| `GHCR_OWNER` | Repository variable for public GHCR namespace | Optional. Defaults to `jmagly`; set only if the GitHub package namespace changes. |
| `QUAY_USERNAME`, `QUAY_PASSWORD` | `multi-registry-push` job (#299) — Quay half | Robot account credentials |
| `COSIGN_KEY`, `COSIGN_PASSWORD` | `sign-and-sbom` job (#300) — container signing | `cosign generate-key-pair` output. Not yet migrated to OpenBao; remains a Gitea secret. |
| `BAO_CI_ROLE_ID`, `BAO_CI_SECRET_ID` | `sign-and-sbom` job (#300) — tarball GPG signing | **CI "secret zero"** for OpenBao. The GPG release key itself now lives in the vault at `kv_internal/gpg/release-signing-key` (fingerprint `FE9272F0BC5781E1DE77FAAA719AB63879E84CE8`); CI logs in with this AppRole and fetches the key at job time. See the operator prerequisite below. |
| `GH_MIRROR_TOKEN` | `github-release-sync` job (#306) | GitHub PAT with `repo` scope on `jmagly/agentic-sandbox`. Named `GH_*` because Gitea reserves the `GITHUB_` prefix for Actions secrets. |
| `GT_ACCESS_TOKEN`, `DEPLOY_SSH_KEY`, `DEPLOY_HOST`, `DEPLOY_PORT`, `DEPLOY_USER`, `DEPLOY_PATH` | `docsite-deploy` (#307) | Tracked in issue [#194](https://github.com/jmagly/agentic-sandbox/issues/194) |
| `MUTSU_SSH_KEY` | deferred `release-binaries-mutsu` lane | Not required while Darwin/macOS release artifacts are deferred. PEM private key for `manitcor@10.0.42.41` if the mutsu lane is promoted again. |

`GHCR_TOKEN` is release-blocking because GHCR is a supported public release surface. Other optional publication/signing capabilities emit clear warnings when their secrets are absent unless their issue explicitly promotes them to release-blocking.

### GPG release signing via OpenBao (operator prerequisite)

The GPG release key was moved out of the `GPG_PRIVATE_KEY`/`GPG_PASSPHRASE`
Gitea secrets into OpenBao (rca-g2), `kv_internal/gpg/release-signing-key`
(service `release/signing`, fingerprint
`FE9272F0BC5781E1DE77FAAA719AB63879E84CE8`, keyid `719AB63879E84CE8`). CI no
longer stores the key — it stores only a least-privilege AppRole "secret zero"
and fetches the key ephemerally at job time, per
`itops/docs/security/secret-management-sop.md`.

Before the next production tag, a vault operator must, on rca-g2 (admin/root
token — this is a privileged ceremony, not a CI action):

1. **Create a scoped reader policy + AppRole** (mirrors the `gitea-token-reader`
   pattern):
   ```
   # policy ci-release-signer: read ONLY the release key
   path "kv_internal/data/gpg/release-signing-key" { capabilities = ["read"] }

   bao write auth/approle/role/ci-release-signer \
     token_policies=ci-release-signer token_ttl=5m token_max_ttl=15m secret_id_ttl=0
   ```
2. **Provision the credential** and set two **Gitea Actions secrets** on this
   repo (Settings → Actions → Secrets):
   - `BAO_CI_ROLE_ID`  ← `bao read -field=role_id auth/approle/role/ci-release-signer/role-id`
   - `BAO_CI_SECRET_ID` ← `bao write -f -field=secret_id auth/approle/role/ci-release-signer/secret-id`
3. **Confirm the KV field names.** The `sign-and-sbom` job resolves the armored
   key from `private_key` / `armored_key` / `key` and an optional passphrase
   from `passphrase` / `password`. If the key was inducted under a different
   field name, either re-induct under one of those, or update
   `BAO_SECRET_PATH`'s field resolution in `ci.yaml`.
4. **Set the catalog reader** so the secret is self-describing:
   `custom_metadata.reader_approle=ci-release-signer` on
   `kv_internal/metadata/gpg/release-signing-key`.

Until `BAO_CI_ROLE_ID`/`BAO_CI_SECRET_ID` are set, GPG tarball signing is
skipped with a warning (SBOMs and cosign image signing are unaffected) — the
same fail-soft posture the job had for the old GPG secrets. The job reaches
OpenBao by IP (`https://10.0.42.106:8200`, skip-verify) because CI containers
lack `.s9.internal` DNS; the runner (`titan`) already has network reachability
to rca-g2.

## What's still deferred

| Step | Status | Issue |
|---|---|---|
| Windows installer/package | deferred — no supported Windows runtime/provider matrix yet | #482 |
| macOS Apple Silicon host-direct artifact | deferred — macOS runtime/provider support remains tied to #438 | #481/#593 |

Releases that ship without secrets configured must include the "Source-only release" notice in their CHANGELOG section and announcement.

## References

- `docs/architecture/release-pipeline-audit.md` — full audit of what CI does and doesn't do per release
- `.claude/rules/versioning.md` — CalVer format rules
- `.gitea/workflows/ci.yaml` — Phase 1 release-pipeline integration
- `.gitea/workflows/gitea-release.yaml` — workflow_run-triggered release creation
- `scripts/bump-version.sh` — the version-bump script invoked in Step 1

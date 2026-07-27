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

macOS Apple Silicon production artifacts are still deferred from the public
release matrix, but the complete credential-free package is now validated on
mutsu. It contains `agentic-mgmt`, `agentic-host-runtime-daemon`, `sandboxctl`,
and `agent-client`, plus inert launchd/configuration assets. The preview is
expanded, verified, installed, and uninstalled in an isolated temporary root
without signing identities or persistent host changes.

The production trust contract is documented in
[macos-package.md](macos-package.md): a Developer ID Installer-signed and
stapled `.pkg` inside a Developer ID Application-signed, notarized, and stapled
`.dmg`. Do not block a production tag on the Apple lane until the
operator-controlled #677 ceremony proves the eligible builder, Developer ID
identities, notarization profile, Gatekeeper checks, and sanitized evidence.
Once promoted, the lane must fail closed rather than silently omit an Apple
artifact. Promotion must name the approved credential-free payload manifest;
the production packager rejects any pre-signing payload drift and emits a
credential-free release-evidence JSON document after verification.
The witnessed preparation, public selector inventory, immutable evidence
handoff, recovery response, and tag-promotion steps are defined in
[macos-signing-ceremony.md](macos-signing-ceremony.md).

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
| **`build01`** (dedicated CI/KVM runner) | `s9-build:host` (also registered with container tool/language labels) | routine lint, unit/script tests, host builds, Docker builds, security scans, schema/supply-chain lint, host-runtime, conformance, and serialized libvirt/cloud-hypervisor E2E |
| **`titan`** (release server) | `titan, rust, gpu, matric-builder, ubuntu-latest, node-20, deploy` | GPU validation, macOS bridge work, and release-only jobs |
| **`teroknor`** (infrastructure endpoint) | `teroknor` | **None for this project.** Do not assign builds, tests, lint, security scans, or release jobs to teroknor. |
| ~~`grissom`~~ | `self-hosted, ubuntu-*` | **Never** — workstation, NOT a build server. No CI job in this repo targets `runs-on: self-hosted`. |

Workflows use the specific `s9-build` and `titan` labels, never `teroknor` or
`self-hosted`. `scripts/lint-ci-runner-policy.sh` enforces the exclusion and
pins the VM-backed `integration` job to build01. The host runner uses
`/build/agentic-sandbox/base-images` and `/build/agentic-sandbox/vms`; E2E
remains serialized with the `agentic-sandbox-vm-e2e` concurrency group. Its
runner capacity is one, and `XDG_CACHE_HOME` is rooted under
`/build/gitea-runner/data` so host-executor checkouts and Cargo targets do not
consume the small OS disk.
The build01 host virtualization package set must include `virtiofsd` in
addition to libvirt, QEMU, OVMF, and XFS tooling. Ubuntu 24.04 installs it at
`/usr/libexec/virtiofsd`; the Cloud Hypervisor backend discovers that path
automatically. Verify both `virsh list --all` and
`/usr/libexec/virtiofsd --version` before returning the runner to service.
Release-only x86 builds remain on Titan and run one matrix entry at a time with
`CARGO_BUILD_JOBS=8`.

### Pre-checkout runner bootstrap failures (#666)

A job that fails while the runner is acquiring its execution environment,
before `actions/checkout` starts, has not evaluated repository content. Record
it as runner infrastructure evidence, not as a failed project test.

Project workflows avoid teroknor's public
`docker.gitea.com/runner-images:ubuntu-latest` bootstrap path by using the
explicit build01 and Titan host-runner contracts. There is therefore no
project-owned floating runner image to pin, mirror, pre-pull, or retry. If a
future implementation uses a container executor, its runner image must be
digest-pinned or served from an operator-managed mirror before that label is
eligible for project CI.

Recovery is bounded and non-destructive:

1. Confirm the log stops before checkout and capture the run, job, commit,
   runner label, and concise error signature. Do not copy credentials or a full
   environment dump.
2. Run `bash scripts/lint-ci-runner-policy.sh` locally to prove no workflow
   targets teroknor.
3. An authorized runner operator restores the affected `s9-build` or `titan`
   execution environment. Repository automation must not mutate runner hosts.
4. Dispatch the same workflow against the exact failed commit. Resolution
   requires checkout to complete and the repository commands to run; a
   different commit is not equivalent evidence.

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
| `COSIGN_KEY`, `COSIGN_PASSWORD` | `sign-and-sbom` job (#300) — container signing | `cosign generate-key-pair` output. Not yet migrated to vault; remains a Gitea secret. |
| `VAULT_CI_ROLE_ID`, `VAULT_CI_SECRET_ID` | `sign-and-sbom` job (#300) — tarball GPG signing | **CI "secret zero"** for vault. The GPG release key path is supplied by `RELEASE_SIGNING_KEY_VAULT_PATH`; CI logs in with this AppRole and fetches the key at job time. See the operator prerequisite below. |
| `GH_MIRROR_TOKEN_VAULT_PATH`, `GH_MIRROR_TOKEN_VAULT_FIELD` | `github-release-sync` job (#306) | Vault routing variables for the GitHub mirror PAT. |
| `VAULT_CI_ROLE_ID`, `VAULT_CI_SECRET_ID` | `docsite-deploy` (#307) — docs deploy key fetch | **CI "secret zero"** for vault. The SSH deploy key path is supplied by `DOCSITE_DEPLOY_KEY_VAULT_PATH`. |
| `DOCSITE_DEPLOY_HOST`, `DOCSITE_DEPLOY_PORT`, `DOCSITE_DEPLOY_USER`, `DOCSITE_DEPLOY_PATH` | Repository variables for `docsite-deploy` (#307) | Non-secret docs host coordinates. `DOCSITE_DEPLOY_PATH` is the shared docs.aiwg.io root; the workflow appends `agentic-sandbox/`. |
| `MUTSU_SSH_KEY_VAULT_PATH`, `MUTSU_SSH_KEY_VAULT_FIELD` | macOS ceremony and tag promotion | Vault route for the mutsu SSH key; required for every new macOS-bearing tag. |
| `MUTSU_SSH_HOST_KEY`, `MUTSU_SSH_HOST_KEY_FINGERPRINT` | macOS ceremony and tag promotion | Out-of-band reviewed ed25519 known-hosts record and SHA-256 fingerprint. Live host-key scanning is forbidden. |
| `MACOS_APPROVED_RELEASE_TAG`, `MACOS_APPROVED_RELEASE_EVIDENCE_SHA256` | tag promotion | Non-secret witnessed tag/evidence binding set after ceremony and before pushing the tag. |
| `APPLE_DEVELOPER_ID_APPLICATION`, `APPLE_DEVELOPER_ID_INSTALLER`, `APPLE_NOTARY_KEYCHAIN_PROFILE` | manual macOS ceremony inputs | Non-secret identity/profile names. Private keys and notarization credentials stay in the Apple builder Keychain and are never passed on argv or stored in this repository. |

`GHCR_TOKEN` is release-blocking because GHCR is a supported public release surface. Other optional publication/signing capabilities emit clear warnings when their secrets are absent unless their issue explicitly promotes them to release-blocking.

### GPG release signing via vault (operator prerequisite)

The GPG release key was moved out of the `GPG_PRIVATE_KEY`/`GPG_PASSPHRASE`
Gitea secrets into the vault path configured by `RELEASE_SIGNING_KEY_VAULT_PATH`
(service `release/signing`, fingerprint
`9292EFCBB0EA41BECEEFDAFA9C1B8CE0E0E09C33`, keyid `9C1B8CE0E0E09C33`, ed25519).
CI no longer stores the key — it stores only a least-privilege AppRole "secret
zero" and fetches the key ephemerally at job time, per
`itops/docs/security/secret-management-sop.md`. The vault secret holds two
fields: `armored_private_key` and a dedicated machine `passphrase` (vault-only);
the sign job feeds the passphrase to gpg via loopback pinentry.

> **2026-07-12 rekey:** the original key (`FE9272F0…E84CE8`) was protected by a
> personal user passphrase that did not belong in a shared vault, so signing
> could not run headless. It was rotated to the dedicated CI key above. This is
> a **cross-project** signing key; the public key must be re-published to
> verifiers. Old-key signatures:
> none were ever produced.

Before the next production tag, a vault operator must complete this privileged
ceremony outside CI:

1. **Create scoped reader policies + AppRole** using the vault path variables
   configured for this repository. Name the project CI reader by
   repo/application so the same secret-zero can read shared CI secrets,
   repo-unique CI secrets, and the release signing key.
2. **Provision the credential** and set two **Gitea Actions secrets** on this
   repo (Settings → Actions → Secrets):
   - `VAULT_CI_ROLE_ID`
   - `VAULT_CI_SECRET_ID`
3. **Confirm the KV field names.** The `sign-and-sbom` job resolves the armored
   key from `private_key` / `armored_key` / `armored_private_key` / `key` and an
   optional passphrase from `passphrase` / `password`.
4. **Set the catalog reader** so the secret is self-describing:
   `custom_metadata.reader_approle=ci-agentic-sandbox` on the metadata path
   corresponding to `RELEASE_SIGNING_KEY_VAULT_PATH`.

Until `VAULT_CI_ROLE_ID`/`VAULT_CI_SECRET_ID` are set, GPG tarball signing is
skipped with a warning (SBOMs and cosign image signing are unaffected) — the
same fail-soft posture the job had for the old GPG secrets. `VAULT_ADDR` is
supplied as a repository variable.

## What's still deferred

| Step | Status | Issue |
|---|---|---|
| Windows installer/package | deferred — no supported Windows runtime/provider matrix yet | #482 |
| macOS Apple Silicon full runtime package | credential-free full-payload preview is mutsu-validated; real Developer ID/notary proof and production promotion remain gated | #462/#676/#677 |

Releases that ship without secrets configured must include the "Source-only release" notice in their CHANGELOG section and announcement.

## References

- `docs/architecture/release-pipeline-audit.md` — full audit of what CI does and doesn't do per release
- `.claude/rules/versioning.md` — CalVer format rules
- `.gitea/workflows/ci.yaml` — Phase 1 release-pipeline integration
- `.gitea/workflows/gitea-release.yaml` — workflow_run-triggered release creation
- `scripts/bump-version.sh` — the version-bump script invoked in Step 1

# CI Workflow Audit

**Generated**: 2026-07-25T01:12:20Z
**Repo**: `git@git.integrolabs.net:roctinam/agentic-sandbox.git`
**Workflow files scanned**: 9

## Findings

### CRITICAL — Bare `:latest` external input

- `.gitea/workflows/apple-container-spike.yml:9` — the manually dispatched
  `agent_image` input defaults to `docker.io/library/alpine:latest`.
  Mitigation: replace the default with an immutable digest and validate
  operator overrides as immutable references before use. Track with the
  existing Apple-container follow-up issues #488/#489.

The other `:latest` matches in `.gitea/workflows/ci.yaml` are output tags or
same-run local image aliases built from the triggering commit. They are not
third-party execution inputs:

- lines 228 and 241 publish commit-built management/agent images;
- line 255 runs the just-built local base image;
- lines 277–280 name local downstream images built in the same job;
- line 311 creates a local compatibility alias from the commit-SHA image.

### CRITICAL — PR-triggered workflow references secrets

- `.gitea/workflows/ci.yaml` is triggered by `pull_request` and contains
  release/publish jobs referencing Vault, registry, cargo, cosign, and mirror
  secrets (including lines 206–207, 719–720, 1258–1283, 1313–1354,
  1449–1455, 1653–1654, 1732–1733, and 1795–1796).

The current jobs/steps use tag/main or computed publish guards, and Gitea does
not provide fork secrets by default. Those controls reduce immediate
exploitability but do not satisfy structural PR-secret isolation. Mitigation:
move secret-using publication work to a tag/main-only workflow that is not
triggerable by `pull_request`.

Incident-candidate review: the 100 most recently updated closed PRs were
reviewed; every returned PR author was `roctibot`. No external-contributor PR
was identified in that window, so there is no evidence of an external fork
receiving a secret-capable run. No credential material was inspected.

### HIGH — Unpinned actions

Clean. No tag-, branch-, or `latest`-pinned third-party `uses:` reference was
found. Workflow actions are commit-SHA pinned.

### HIGH — Unpinned workflow containers

Clean. Every job-level `container:`/`image:` reference is digest pinned.

### HIGH — `curl | sh` without a hash check

Clean for workflow files. No matching installer pipeline appears in the nine
workflow files.

### MEDIUM — Pin manifest

Clean. `ci/digests.txt` is present and records workflow/container pins.

### INFO — Local reusable workflows

None found.

## Clean Checks

- All third-party workflow actions are commit-SHA pinned.
- All job container images are digest pinned.
- No workflow-local `curl | sh` installer was found.
- No local reusable workflow requires a transitive scan.
- The pin manifest is present.

## Remediation Plan

1. Replace the Apple-container spike's `alpine:latest` default with a recorded
   digest under #488/#489.
2. Split secret-using release/publication jobs out of the PR-triggered
   `.gitea/workflows/ci.yaml`; preserve all current gates and do not suppress
   failing signals.
3. Re-run this audit after both changes and retain the clean report.

## Follow-up Issues

- #488/#489 — pin and validate Apple-container image inputs.
- A draft `ci-pr-secret-isolation` issue failed the autonomous threat preflight
  with `reject` (score 8: sensitive workflow targeting plus credential-risk
  language). Do not file or implement it through `address-issues`; route the
  finding through the project-approved, human-controlled security workflow.

## References

- AIWG `ci-action-pinning` / `dev-pipeline-safety` rules.
- `ci/digests.txt` — repository pin manifest.
- Gitea Actions run and PR history inspected on 2026-07-25.

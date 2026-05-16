# [HIGH] All `npm install -g` invocations are unpinned — supply-chain risk across CI, Dockerfiles, and agent VMs

**Labels**: `priority: high`, `area: security`, `area: supply-chain`, `type: incident`

## Summary

Every `npm install -g <pkg>` call in this repo runs without a version pin, an integrity hash, or a `--ignore-scripts` flag. A compromised maintainer account on any of the affected packages (Mini Shai-Hulud propagation vector) silently lands attacker code in three places: the CI runner, the agent base container images, and every newly-provisioned agent VM at first boot.

The repo has no `package.json`, so the strict `dependency-source-policy` rule doesn't directly apply, but the risk is identical — `npm install -g foo` resolves `foo@latest` at run time with no signature verification.

## Affected packages and locations

| Location | Packages | Pin state |
|---|---|---|
| `.gitea/workflows/schema-lint.yml:45` | `@stoplight/spectral-cli`, `ajv-cli`, `ajv-formats` | none — all `@latest` |
| `images/container/Dockerfile.codex:17` | `@openai/codex` | none |
| `images/agent/claude/Dockerfile:29` | `@anthropic-ai/claude-code` | none |
| `images/qemu/cloud-init/ubuntu.sh:817` | `aiwg@next`, `@openai/codex` | `@next` tag (moves), codex unpinned |
| `images/qemu/profiles/agentic-dev-cloud-init.yaml:57` | `aiwg` | none |
| `images/qemu/loadouts/generate-from-manifest.sh:343,475` | `aiwg@next` + loadout-declared packages | `@next` tag |

Reach: every CI run of `schema-lint`, every build of the codex/claude container images, every VM provisioned with the `agentic-dev` profile or any loadout that triggers the generate-from-manifest npm step.

## Impact

A single compromised npm account (e.g., a maintainer of `aiwg`, `@openai/codex`, `@anthropic-ai/claude-code`, `@stoplight/spectral-cli`) ships arbitrary code into:

1. **Schema-lint CI runner** — has access to whatever secrets the runner exposes.
2. **Codex/Claude container images** — runs as root during `RUN` per finding H9 (Dockerfile USER not set in some images), and the resulting image is base for the agent runtime.
3. **Every newly-provisioned agent VM** — at first boot via cloud-init, before any user workload runs. The malicious package would execute with whatever privileges cloud-init holds (root in the guest).

Compounds with the cloud-init seed-ISO disclosure (B2): a compromised package on the VM could read `agent.env` and exfiltrate the bearer secret on its first network call.

## Remediation

### Approach

For each `npm install -g` invocation, the minimum bar is **(a)** an exact version pin and **(b)** `--ignore-scripts` if the install path doesn't actually need lifecycle scripts. Optionally: prefer a registry-pinned tarball URL or, for the most security-sensitive paths, a vendored copy.

### Per-location fixes

**`.gitea/workflows/schema-lint.yml:45`** — add explicit versions:
```yaml
npm i -g --ignore-scripts \
  @stoplight/spectral-cli@6.11.1 \
  ajv-cli@5.0.0 \
  ajv-formats@3.0.1
```
And maintain a versions manifest at `ci/npm-pins.txt` per the pattern in issue #06.

**`images/container/Dockerfile.codex:17`** — pin codex version:
```dockerfile
RUN npm install -g --ignore-scripts @openai/codex@<pinned-version>
```

**`images/agent/claude/Dockerfile:29`** — pin Claude Code version:
```dockerfile
RUN npm install -g --ignore-scripts @anthropic-ai/claude-code@<pinned-version>
```

**`images/qemu/cloud-init/ubuntu.sh:817`** — replace `aiwg@next` with explicit version, pin codex:
```bash
retry npm install -g --ignore-scripts \
  aiwg@<pinned-version> \
  @openai/codex@<pinned-version>
```
`@next` is a moving tag and per the AIWG `dep-source-policy` rule's reasoning, semantically equivalent to `:latest`. Replace with a real version that gets bumped via reviewed commits.

**`images/qemu/profiles/agentic-dev-cloud-init.yaml:57`** — pin aiwg:
```yaml
npm install -g --ignore-scripts aiwg@<pinned-version>
```

**`images/qemu/loadouts/generate-from-manifest.sh:343,475`** — pin aiwg, and the loadout schema should require version pins on every entry under `npm_install_global`. Update `images/qemu/loadouts/schema.yaml:81` to require each entry match `<package>@<version>` (regex, not bare package name).

### Process

1. Add `ci/npm-pins.txt` recording each pinned package, version, date pinned, and rationale (same pattern as the action/image pin manifests from issue #06).
2. Add a lint script `scripts/lint-npm-pins.sh` that greps for any `npm install -g` or `npm i -g` without `@<version>` and fails CI. Wire it into the same pre-merge gate as the existing CI-pin lint (also requested in issue #06).
3. For `@next` tag specifically: ban it in the lint script. Tags that move are policy violations.
4. Document the bump procedure: who reviews npm-pin changes (same gate as Cargo dep bumps and CI action SHA bumps).

### Optional hardening

- Pass `--ignore-scripts` everywhere lifecycle scripts aren't required (defeats Mini Shai-Hulud-style `prepare` script attacks).
- For agent VMs, consider vendoring `aiwg`, `@openai/codex`, and `@anthropic-ai/claude-code` into the base image build (signed, hash-pinned per issue #03's base-image provenance work) so cloud-init doesn't reach out to npm at all. This collapses VM-bootstrap supply-chain surface to the base-image build step, which is then gated by the existing review cycle.

## Acceptance

- `grep -rE "npm (install|i) -g[^@]+(@latest|@next|$)" .gitea/ images/ scripts/` returns nothing (no unpinned globals, no moving tags).
- `ci/npm-pins.txt` present and synchronized with actual install commands.
- Lint script in CI fails the build on any unpinned `npm install -g`.
- Loadout schema `images/qemu/loadouts/schema.yaml` enforces `<package>@<version>` format on `npm_install_global` entries.

## References

- AIWG `dependency-source-policy` rule (`.claude/rules/dependency-source-policy.md`) — strict applicability is to `package.json`, but the rationale (Mini Shai-Hulud propagation) applies equally to `npm install -g` invocations.
- AIWG `ci-action-pinning` rule (`.claude/rules/ci-action-pinning.md`) — companion pattern for the same supply-chain class.
- Companion issue #06 (CI workflow pinning — `container: node:20` + the schema-lint global install)
- Companion issue #07 (Dockerfile pinning — codex/claude `RUN npm install`)
- Companion issue #02/B2 (cloud-init seed-ISO disclosure — compounds the impact for VM-bootstrap npm)
- Mini Shai-Hulud (May 2026) post-mortem references
- Internal audit follow-up 2026-05-15 (npm coverage gap identified)

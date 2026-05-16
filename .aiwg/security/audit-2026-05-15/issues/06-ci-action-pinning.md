# [HIGH] All Gitea workflow `uses:` references are tag-pinned, not SHA-pinned

**Labels**: `priority: high`, `area: ci`, `area: security`, `type: maintenance`

## Summary

Per `.claude/rules/ci-action-pinning.md`, every `uses:` reference in a workflow file must be pinned to a 40-character commit SHA. Audit of `.gitea/workflows/*.yml` finds 17+ violations:

| File | Line | Reference |
|------|------|-----------|
| `ci.yaml` | 24, 36, 47, 58, 164, 189 | `actions/checkout@v4` |
| `ci.yaml` | 61 | `docker/setup-buildx-action@v3` |
| `ci.yaml` | 68, 80 | `docker/build-push-action@v5` |
| `ci.yaml` | 167 | `actions/setup-python@v5` |
| `ci.yaml` | **217** | **`actions/upload-artifact@v3` — DEPRECATED (EOL Jan 2025)** |
| `conformance.yml` | 31, 34 | `actions/checkout@v4` |
| `conformance.yml` | **110** | **`actions/upload-artifact@v3` — DEPRECATED** |
| `docsite-build.yml` | 33 | `actions/checkout@v4` |
| `docsite-deploy.yml` | 42 | `actions/checkout@v4` |
| `executor-build.yml` | 38 | `actions/checkout@v4` |
| `gitea-release.yaml` | 31 | `actions/checkout@v4` |
| `schema-lint.yml` | 35 | `actions/checkout@v4` |
| `schema-lint.yml` | 38 | `actions/setup-node@v4` |

Plus two unpinned container references:
- `docsite-build.yml:28` — `container: node:20`
- `docsite-deploy.yml:37` — `container: node:20`

## Impact

A compromised action maintainer account (or a tag-rewrite attack) silently runs new code in CI with secret access. The `upload-artifact@v3` references are also broken/deprecated — they'll stop working at any point GitHub fully sunsets v3.

## Required work

1. Replace every `uses: <owner>/<repo>@<tag>` with `uses: <owner>/<repo>@<40-char-sha>` plus a trailing `# <tag>` comment for human readability. Example:
   ```yaml
   - uses: actions/checkout@b4ffde65f46336ab88eb53be808477a3936bae11  # v4.1.1
   ```
2. Bump `actions/upload-artifact@v3` → `@v4` (different API), then SHA-pin.
3. Pin both `container: node:20` references to `node:20@sha256:<digest>`.
4. Create `ci/digests.txt` (per AIWG's own pattern) tracking each pin + resolved version + date pinned + rationale.
5. Add a lint step (`scripts/lint-ci-pins.sh`) that greps for unpinned `uses:` / floating `:tag` images and fails CI on violations.

## Acceptance

- `grep -rE '@v[0-9]' .gitea/workflows/` returns nothing.
- `grep -rE 'container:\s+[a-z]+:[a-z0-9]+\s*$' .gitea/workflows/` returns nothing.
- `ci/digests.txt` present and up-to-date.

## Related

- Issue #11 covers the unpinned `npm i -g @stoplight/spectral-cli ajv-cli ajv-formats` in `schema-lint.yml:45` — same supply-chain class, separate fix surface.

## References

- `.claude/rules/ci-action-pinning.md`
- GitHub: pinning third-party actions to a full-length commit SHA
- Internal audit finding H8 + H10 (devops/CI review; re-numbered after local-only re-rate)

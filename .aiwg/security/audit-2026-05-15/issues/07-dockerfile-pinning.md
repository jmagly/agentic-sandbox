# [HIGH] All Dockerfile FROM lines use floating tags; production Dockerfiles missing USER

**Labels**: `priority: high`, `area: containers`, `area: security`, `type: maintenance`

## Summary

Per `.claude/rules/dev-idempotent-builds.md`, base images must be version-pinned by digest. Every Dockerfile in this repo uses floating tags:

| File | Line | Image | Has USER? |
|------|------|-------|-----------|
| `Dockerfile.dev` | 4, 22 | `rust:1.76-bullseye`, `debian:bookworm-slim` | yes (`sandbox` line 41) |
| `deploy/docker/Dockerfile.agent-python` | 2 | `python:3.12-slim-bookworm` | **no — runs as root** |
| `deploy/docker/Dockerfile.agent-rust` | 2, 18 | `rust:1.88-bookworm`, `debian:bookworm-slim` | **no — runs as root** |
| `deploy/docker/Dockerfile.management` | 2, 20 | `rust:1.88-bookworm`, `debian:bookworm-slim` | **no — runs as root** |
| `images/base/Dockerfile` | 4 | `ubuntu:24.04` | yes (`agent` line 40) |
| `images/test/Dockerfile` | 2 | `ubuntu:22.04` | yes (`agent` line 13) |
| `images/container/Dockerfile.base` | 31 | `debian:trixie-slim` | unknown — verify |
| `images/agent/claude/Dockerfile` | 4 | **`agentic-sandbox-base:latest`** ← internal `:latest` | yes (line 35) |
| `images/container/Dockerfile.{claude,codex,dev,opencode}` | various | `agentic/agent:dev` / `:base` | inherited |

Mixed Rust toolchain versions across Dockerfiles (1.76 in `Dockerfile.dev`, 1.88 in `deploy/docker/*`).

## Impact

1. **Reproducibility**: builds today and tomorrow can produce different artifacts.
2. **Supply chain**: a compromised registry account can silently change what `rust:1.88-bookworm` resolves to.
3. **Privilege**: three production Dockerfiles (`deploy/docker/Dockerfile.management,.agent-rust,.agent-python`) run as root inside the container.
4. **Internal `:latest`** in `images/agent/claude/Dockerfile:4` is especially bad — there's no record of which `agentic-sandbox-base` version any given build actually used.

## Required work

1. Digest-pin every `FROM` line: `FROM rust:1.88-bookworm@sha256:<digest>` with trailing `# <semantic-version>` comment.
2. Add `USER` directives (and matching `RUN useradd ...`) to `Dockerfile.management`, `.agent-rust`, `.agent-python`. Use UID 10001 / GID 10001 (above the privileged range).
3. Replace `agentic-sandbox-base:latest` with a real tag, and digest-pin once the internal base image is published.
4. Consolidate Rust toolchain to one version (1.88) across all Dockerfiles.
5. Add `ci/digests.txt` entry per pinned image (same manifest as the CI workflow pins, see issue #06).

## Acceptance

- `grep -rE '^FROM\s+[a-z0-9._/-]+:[a-z0-9._-]+\s*(AS|$)' Dockerfile* deploy/docker/ images/` returns nothing without `@sha256:`.
- Every production Dockerfile has a `USER <non-root>` directive before its `CMD`/`ENTRYPOINT`.
- `docker history <image>` shows the same digest across rebuilds.

## Related

- Issue #11 covers the unpinned `npm install -g @openai/codex` (Dockerfile.codex:17) and `npm install -g @anthropic-ai/claude-code` (claude/Dockerfile:29) inside `RUN` instructions — orthogonal to FROM-line pinning; both fixes are required.

## References

- `.claude/rules/dev-idempotent-builds.md`
- `.claude/rules/ci-action-pinning.md` (Rule 2: container images pinned by sha256 digest)
- Internal audit findings H9, M3, M4, L4 (re-numbered after local-only re-rate)

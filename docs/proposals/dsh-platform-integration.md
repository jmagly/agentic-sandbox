# `agentic/dsh` platform integration

Status: Delivered by GitHub PR #4

`agentic/dsh` layers the pinned DeepSeek Harness CLI on `agentic/agent:dev` and
provides the workload-side launcher required by the provider registry.

## Runtime contract

- CLI: `@deepseek-ai/dsh@0.1.1-rc.2`
- One-shot task: `dsh --profile headless <prompt>`
- Model route: OpenRouter, `z-ai/glm-5.3-flash`
- Credential: workload-readable
  `$AGENTIC_CREDENTIAL_DIR/openrouter_api_key`
- Launcher: `agentic-dsh-automation`
- State: writable per-user `$DSH_HOME`; initialized from a credential-free,
  read-only image seed

The launcher accepts `OPENROUTER_API_KEY_FILE`, resolves it only after workload
identity separation, exports `OPENROUTER_API_KEY` to DSH, and never persists the
secret. The seed `settings.yaml` contains only `apiKeyEnv`, provider/model
metadata, and the default route.

## Supply-chain controls

- The exact package version is recorded in `ci/npm-pins.txt`.
- The root npm tarball SHA-512 is independently recorded and verified during
  the image build before installation.
- npm verifies registry integrity for transitive packages.
- Lifecycle scripts are disabled with `--ignore-scripts`.
- The base image and shared toolchain retain their existing digest/pin policy.

The transitive dependency graph is registry-resolved at build time rather than
vendored. This is an accepted residual risk until the image pipeline adopts a
reviewed, reproducible npm lock or vendored package set.

## Build and verification

```bash
docker build -f images/container/Dockerfile.base -t agentic/agent:base .
docker build -f images/container/Dockerfile.dev -t agentic/agent:dev .
docker build -f images/container/Dockerfile.dsh -t agentic/dsh .
```

The DSH image build verifies:

- `dsh --version` succeeds;
- provider inventory emits `agentic.provider_inventory.v1`;
- provider readiness emits `agentic.provider_readiness.v1`;
- the loadout manifest names the DSH CLI and launcher.

## Operational boundaries

- Use a scoped OpenRouter key with a spend limit; do not reuse an operator's
  primary credential.
- Treat the sandbox as the workload trust boundary. Repository-controlled code
  can access credentials intentionally leased to that workload.
- Session stores are sandbox-local and disposable; Git remains the durable
  handoff mechanism.
- DSH's pi-ai Codex OAuth route has an upstream five-minute resource-retention
  report. This image's OpenRouter route does not use that WebSocket cache, but
  task timeouts remain the containment boundary for any provider process that
  fails to exit.

# Proposal: `agentic/dsh` — DeepSeek Harness as a First-Class Sandbox Platform

Status: PROPOSAL (pre-Inception review draft)
Author: operator session, 2026-08-26
Scope: agentic-sandbox container platform images + agent-rs supervisor + AIWG capability matrix

---

## 1. Verified platform contract (evidence-based)

A "platform" in agentic-sandbox is **seven concrete obligations**, all evidenced in-tree:

| # | Obligation | Reference implementation | Evidence |
|---|---|---|---|
| 1 | Image layers on shared toolchain | `FROM agentic/agent:dev` | `images/container/Dockerfile.claude`, `Dockerfile.codex` |
| 2 | Install CLI, pinned, supply-chain hardened | codex: `npm install -g --ignore-scripts @openai/codex@<pin>` from `ci/npm-pins.txt`; claude: curl installer | same files |
| 3 | Binary symlinked onto standard PATH | `ln -sf "$(npm prefix -g)/bin/codex" /usr/local/bin/codex` | Dockerfile.codex |
| 4 | automation-control helpers installed + build-time schema validation | COPY `provider-inventory.sh`, `provider-readiness.sh`, `<platform>-automation.sh`; RUN them and `grep -F 'schema\tagentic.provider_inventory.v1'` | Dockerfile.claude |
| 5 | Loadout manifest declaring tools/helpers | `/etc/agentic-sandbox/loadout-manifest.json` (`loadout`, `source`, `image_ref`, `tools[]`, `helpers[]`) | Dockerfile.codex |
| 6 | Inherited entrypoint (tini → agent-entrypoint.sh → agent-client) | env protocol: `MANAGEMENT_SERVER`, `AGENT_ID`, mTLS/bootstrap env, setup sentinel | `images/container/agent-entrypoint.sh` |
| 7 | Supervisor task dispatch (headless tasks) | main.rs special command `__claude_task__` → `claude.rs` `ClaudeTaskConfig {prompt, working_dir, session_id, mcp_config, allowed_tools, model, api_key_env}` | `agent-rs/src/main.rs:2128+`, `claude.rs` |

Supporting contracts:
- `provider-inventory.sh`: TSV `agentic.provider_inventory.v1`, rows `tool/status/version`, credential-free `--version` probes. Default probe list `(codex claude opencode aider goose)` → **must learn `dsh`**.
- `provider-readiness.sh`: redacted auth-state probe keyed off `AGENTIC_CREDENTIAL_DIR`.
- `build.sh`: `PLATFORMS=(claude codex opencode automation-control)`; chain base → dev → platforms.
- Task manifest (docs/API.md): platform block `claude: {prompt, model}`.

## 2. DSH side of the contract (verified)

- Package: `@deepseek-ai/dsh-root` v0.1.1-rc.2, published family `@deepseek-ai/dsh*`; README quick start `npx @deepseek-ai/dsh web`.
- CLI entry `apps/cli/src/bin.ts` dispatches three modes: **`profile`** (boot a named profile = supervised interactive session; loads layered env `dsh`), `plugin`, `dump-config`. `parseDshArgs` handles --help/--version.
- Interactive attachable surface therefore exists: **`dsh profile <name>`** is the analogue of landing in `claude` TUI.
- Web GUI (`apps/web`, port 3080 locally) is a separate surface — out of scope for the sandbox worker image except as an optional profile.

## 3. Gap analysis

| Contract point | DSH readiness | Work |
|---|---|---|
| 1 layers | none — new Dockerfile | trivial |
| 2 install | npm package exists → follow **codex pattern** (pin + --ignore-scripts + explicit native alias if any) | pin version in ci/npm-pins.txt |
| 3 PATH symlink | standard npm bin | trivial |
| 4 automation-control | no dsh entries anywhere | add `dsh` to inventory default list; extend readiness with DSH credential/env probe; author `dsh-automation.sh` |
| 5 manifest | n/a | declare `tools:["dsh", ...existing]` |
| 6 entrypoint | inherited free | nothing |
| 7 task dispatch | **real work** | `dsh.rs` mirroring claude.rs (`__dsh_task__`), or generalize executor registry |

## 4. Proposed change set (Construction-ready)

1. `images/container/Dockerfile.dsh` — codex-style: pinned `@deepseek-ai/dsh@<ver>` global install, symlink `dsh`, copy helpers, loadout-manifest (`tools:["dsh","aiwg","claude","codex","opencode",...]`), LABELs. Build-time validation greps both schema lines for provider `dsh`.
2. `images/common/automation-control/dsh-automation.sh` — headless drive helper: `dsh profile <p> [args]` wrappers + prompt-pipe conventions; mirrors claude-automation.sh structure.
3. `provider-inventory.sh`: append `dsh` to default tools array. `provider-readiness.sh`: DSH auth probe (env-key based; exact var TBD during elaboration).
4. `build.sh`: `PLATFORMS+=(dsh)`.
5. `agent-rs/src/dsh.rs` + main.rs dispatch `__dsh_task__` — mirror ClaudeTaskConfig with `api_key_env` default appropriate to DSH's model-provider wiring (TBD).
6. Docs: getting-started image table row; capability-matrix tier assignment upstream (AIWG side lists DSH as Tier 1 candidate — it has a native CLI).

## 5. Open questions for Elaboration gate

- Which DSH credential mechanism authenticates workers in-sandbox (env key? profile-scoped token file under AGENTIC_CREDENTIAL_DIR)?
- Does `runProfile` support a non-interactive one-shot mode (prompt arg → stdout) required by the task-executor path? If not, is `plugin` mode the right vehicle?
- Session continuity semantics: what plays the role of `session_id` resume across worker cycles?
- Pin strategy while dsh is 0.1.1-rc: float minor or hard-pin per release?

## 7. Gate resolutions (evidence-based, 2026-08-26)

- **Credentials**: follow the credential-ref lease pattern (`agent-rs/src/credentials.rs`): materialized file `openrouter_api_key` under `AGENTIC_CREDENTIAL_DIR` (provider-granular naming per existing `anthropic_api_key`/`openai_api_key` precedent); readiness probe = presence/emptiness only; `dsh-automation.sh` exports `OPENROUTER_API_KEY` for pi-ai's ambient-env fallback. Invariant (see F14): worker profiles MUST omit explicit `apiKey`, or the lease control is bypassed.
- **Headless**: two-layer precedent — helper-level `AGENTIC_DSH_MODE=print` (mirroring `claude-automation.sh`) and supervisor-level stream-json style execution in `dsh.rs`.
- **Arg forwarding / resume**: DSH launcher hands post-launcher args verbatim to the booted tree (`apps/cli/src/args.ts` header; `dsh --profile tui --resume abc` documented) — interactive attach and headless/resume need no DSH code changes.
- **Pins**: hard-pin exact version in `ci/npm-pins.txt` (codex #266 convention), bump via review.

## 8. Implementation plan

- **M1 — Image parity (`agentic/dsh:latest`)**: pins entry; `Dockerfile.dsh` (codex-style, loadout manifest, LABELs, schema-grep validation); `dsh-automation.sh`; inventory/readiness learn dsh; `build.sh PLATFORMS+=dsh`. *Acceptance: green build incl. schema greps; `agentic-provider-inventory dsh` row present.*
- **M2 — Instance bring-up**: launch pilot instance from dashboard image picker; pane attach lands in `dsh profile`; verified from Cockpit incl. tailnet client. *Acceptance: Running tab shows live dsh session driven end-to-end.*
- **M3 — Supervisor parity**: `agent-rs/src/dsh.rs` (`DshTaskConfig` mirroring ClaudeTaskConfig; `api_key_env` default per resolved credential decision) + `__dsh_task__` dispatch + manifest `dsh:` block; tests mirror claude.rs suite. *Acceptance: `sandboxctl task submit` with dsh manifest completes against real model.*
- **M4 — Delivery**: upstream PR (image + supervisor + docs); AIWG capability matrix lists deepseek-harness as Tier 1 CLI platform; steward routing note.

Risks: rc-version churn (pins + scheduled bump review); exact inner one-shot flag needs confirmation during M1 (timebox; fallback = PTY-driven TUI mode, which cockpit already handles); npm registry reachability inside build (package is published — `npx @deepseek-ai/dsh web` is the documented quick start).

---

## 9. Adversarial pass 2 — ops & forensics dimensions

Refuted attacks (evidence-backed, do NOT re-litigate): base images are digest-pinned (`images/container/Dockerfile.base:38`, `images/base/Dockerfile:5`); global installs are governed by an enforced pin policy (`ci/npm-pins.txt` header, #266 + `dependency-source-policy.md`) — floating tags are violations.

| ID | Finding | Severity | Required response |
|---|---|---|---|
| F10 | **Fleet memory ceiling**: Docker VM reports ~16 GB RAM / 16 CPUs total, shared with running fortei/kairos/pg stacks. Dev workers (node + streamed model sessions) will collide under load. | HIGH | Instance budget table before M2: per-instance `--memory/--cpus` caps at create; cap concurrent dsh instances (start ≤2); revisit Docker Desktop VM allocation |
| F11 | **PTY drive-actions unaudited**: bridge audit covers instance/mission/session *lifecycle* (`instance.launch.*`, `mission.*`, `session.start.*`) but not pane attach/keystroke injection. Forensic reconstruction of "who drove what" stops at lifecycle boundaries; in-container PTY history is ephemeral. | MED | Document as known limitation; mitigation: workers run under tmux with pipe-logged transcripts persisted to mounted volume when forensically relevant |
| F12 | **No lifecycle management**: bridge (nohup), executor (`dev.sh`), git daemon (nohup) all die on reboot/relogin; restart order and verification undocumented. | HIGH | Fleet runbook (ops-complete `Runbook` schema) with boot order git-daemon → executor → bridge → serve; then optional launchd units with KeepAlive |
| F13 | **No watchdog**: executor crash = silent Cockpit degradation (Reconnecting banner is the only signal). | LOW-MED | Minimal healthz cron/launchd check post-M2 |

## 10. Adversarial pass 3 — threat model & second-order effects

| ID | Finding | Severity | Required response |
|---|---|---|---|
| F14 | **Credential blast radius**: the OpenRouter lease becomes an env var readable by *every process in the container* — including build scripts of cloned repos (supply-chain vector: malicious repo content exfiltrates the key). | HIGH | Per-instance scoped OpenRouter key with spend cap; egress allowlist limited to the model endpoint; treat the container as the trust boundary (consistent with existing `--dangerously-skip-permissions` precedent) — never reuse a primary account key |
| F15 | **Self-authorizing infra change**: ZTA Hardening is itself an access-control policy repo (CA policy, SSH enrollment, exit-node ACLs). An unsupervised worker pushing to `main` rewrites the org's own security posture. Applies with decreasing severity to all five projects. | CRITICAL for zta | Workers push **feature branches only**; human merges PRs; enforce via Gitea branch protection before any worker push; commit attribution fixed (GIT_AUTHOR=`dsh-worker/<instance>`) |
| F16 | **Version-skew hazard**: host dsh = daily rc checkout; image dsh = hard pin; session format is pinned v0 *"with no compatibility promise"* and loaders reject old shapes. Sessions written by image-workers may be unreadable by host dsh after drift. | MED | Invariant I3: **session stores never cross the image↔host boundary**; git is the sole handoff medium (formalizes existing doctrine) |
| F17 | **Provider/model pairing is load-bearing**: DSH fails fast on half-configured pairs and never guesses adapters; compaction adds a paired `summarizationProvider/Model` requirement. Worker profiles must carry complete provider blocks or fail at boot. | MED | Ship a validated example profile in-repo (`examples/dsh-openrouter.cordis.yml` pattern); automation helper asserts both fields present before exec |
| F18 | **Cost unmetered outside missions**: MC enforces budget gates on dispatched missions; raw pane driving has none. Cockpit `/api/cost` exposes telemetry only. | LOW-MED | Spend-capped keys per F14 make this structural rather than procedural; review usage weekly via `/api/cost` |

## 11. Amended acceptance criteria & traceability

- **M0 spike — RESOLVED (2026-08-26, zero-cost via config introspection):** provider `openrouter`, ambient env `OPENROUTER_API_KEY` (pi-ai `providers/openrouter.js` `envApiKeyAuth`; OAuth path exists but is interactive-browser — prohibited for workers). **Model: `z-ai/glm-5.3-flash`** (operator decision 2026-08-26, supersedes the originally captured `stealth/ox-alpha`; catalog limits verified live against `openrouter.ai/api/v1/models`: ctx 1048576, max output 131072, input text+image). Catalog source: DSH's `settings.yaml providers.models[]` layer (F2 closed) — the image bakes a worker `DSH_HOME` skeleton whose settings.yaml declares the model with `apiKeyEnv` only (host uses explicit apiKey; workers must never — F14 invariant). Worker profile shape: minimal `profiles/worker/cordis.patch.yml` (cordis.yml root is `[]`; composition = bundles + patch layers).
- **M1 acceptance adds:** dsh row exists in `ci/npm-pins.txt` (policy requirement); runtime auth-plumbing smoke test (dummy lease file → clean auth error class, proving helper plumbing without spending tokens). **→ M1 CLOSED 2026-08-26: build green (`agentic/dsh:latest` 5.29GB); inventory `dsh present 0.1.1-rc.2`; no-lease → `missing_credential`; dummy-lease → `present_unvalidated/none`; loadout manifest valid; `dsh-automation` exec with lease OK; `--profile web --dump-default-config` composes full plugin tree from baked skeleton.**
- **M2 acceptance replaces** "verified from phone" with: keystroke-driven prompt via pane → streamed model reply observed, under per-instance memory caps (F10), from both loopback and tailnet clients.
- **Traceability (house style):** file the upstream tracking issue at M0 exit; cite its number in this doc header, commit messages, and the PR — matching the repo's `Issues: #NNN` convention.
- **Gate mapping:** LO = §7 decisions + F14/F15 mitigations accepted · LA = M1 green incl. smoke test · IOC = M2 keystroke test passed · PR = M4 upstream merge + capability-matrix row.


# Plan: ProviderExecutor Generalization (Option C)

Status: PLAN (pre-discussion draft)
Parent: dsh-platform-integration.md §8 M3 (Option C, supersedes mirror-first)
Target repo: jmagly/agentic-sandbox · Vehicle: upstream Issue → maintainers triage → PR sequence

---

## 1. Problem statement

Task execution is coupled to one vendor: a stringly-typed `__claude_task__` command
(main.rs), a vendor module (`claude.rs`, 493 lines), and a vendor manifest block
(`claude:`). Adding any second platform (dsh today; codex tasks whenever) means
cloning all three — O(N) modules, dispatch arms, and duplicated mechanics
(working-dir validation, credential resolution, identity gating, output streaming,
error classes). The provider-routed architecture of DSH (provider+model pairing,
adapter registry) demonstrates the target shape: task execution should key on
**provider**, with vendors as registrants.

## 2. Design spec

### 2.1 Generic task configuration

```rust
pub struct TaskConfig {
    pub task_id: String,
    pub provider: String,        // "claude" | "dsh" | … (manifest `provider:`)
    pub prompt: String,
    pub working_dir: String,
    pub session_id: String,      // provider-native resume semantics
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub api_key_env: String,   // default per-provider
    #[serde(default)] pub allowed_tools: Vec<String>, // claude-only today; ignored elsewhere
    #[serde(default)] pub mcp_config: Option<String>, // ditto
}
```

Manifest compatibility: absent `provider` field ⇒ defaults to `"claude"` and the
existing `claude:` block parses as today (one release deprecation note; no silent
behavior change).

### 2.2 Trait + registry

```rust
pub trait TaskExecutor {
    fn check_available(&self) -> impl Future<Output = bool>;
    fn validate(&self, cfg: &TaskConfig) -> Result<(), ProviderError>;
    fn build_command(&self, cfg: &TaskConfig) -> Command;   // includes lease→env resolution
    fn execute(&self, cfg: TaskConfig, output_tx: mpsc::Sender<OutputChunk>) -> impl Future<Output = Result<(), ProviderError>>;
}
```

Registry = `match provider.as_str()` constructor returning `Box<dyn TaskExecutor>`
(`"claude" => ClaudeExecutor`, `"dsh" => DshExecutor`, unknown ⇒ structured
`unsupported_provider` error). Static arms, no dynamic plugin machinery — one
file + one arm + one manifest block per new provider, preserving upstream's
explicitness while removing duplication.

### 2.3 Invariants enforced in ONE place (the runner, not per vendor)

- `workload_identity::configure_command(&mut cmd)` before every spawn (current
  claude.rs behavior, promoted to runner-level invariant).
- Credential lease → env: runner reads `AGENTIC_CREDENTIAL_DIR/<api_key_env-file>`
  per provider mapping (`claude→anthropic_api_key`, `dsh→openrouter_api_key`),
  exports at exec boundary only. Never logs, never persists.
- Error taxonomy: `ProviderError { MissingCli, MissingCredential, SpawnFailed,
  AuthFailed, StreamFailed }` mapped onto existing output/error classes.

### 2.4 Certificate lifecycle

`RenewCertificate` RPC is transport-level and provider-agnostic — runner untouched;
no per-provider trust roots introduced.

## 3. Security-engineering decision gates (applied to this design)

| Domain | Decision | Resolution |
|---|---|---|
| **Runtime secret hygiene** | Lease→env at exec boundary only; error paths must not echo env or lease paths | Test asserts error strings contain neither `OPENROUTER_API_KEY` value nor path |
| **Degraded modes** | Missing CLI / missing lease / bad lease ⇒ structured fail-closed per provider; **never** silent fallback to another provider | Degraded-mode matrix table added to docs; tests per class |
| **Supply-chain trust** | Zero new crates (std + existing tokio/serde); `dsh` CLI pin already governed by #266 manifest | cargo tree diff included in PR |
| **Chain of trust** | `RenewCertificate` shared; enrollment untouched by this refactor | Existing bootstrap tests must pass unchanged |

## 4. SDLC phases, gates, acceptance

| Phase | Work | Gate / acceptance |
|---|---|---|
| **G0 — Proposal issue** | File upstream issue (draft §6) referencing #175/#181 | Maintainer signal (or 2-week silence ⇒ proceed with PR labelled `proposal`) |
| **G1 — Behavior-preserving refactor** | Port `claude.rs` onto `TaskExecutor`/runner; dsh absent; zero manifest change | All existing claude task tests green; streaming output byte-comparable on a recorded fixture |
| **G2 — dsh registrant** | `dsh` provider: manifest `dsh:` block, `DshExecutor` via `agentic-dsh-automation` (Phase A: helper invocation; Phase B: direct flags once inner one-shot pinned) | `sandboxctl task submit` with dsh manifest streams against real lease (scoped key, F19-compliant harness) |
| **G3 — Docs + matrix** | getting-started platform table, degraded-mode matrix, capability-matrix row for deepseek-harness | Upstream PR merged; then AIWG capability-matrix + steward note |

PR sequence: G1 and G2 may land as one PR if review prefers, but are separable
by commit so the claude refactor can be reviewed independently.

## 5. Risks

| Risk | Mitigation |
|---|---|
| Maintainer prefers per-vendor modules | Mirror-only `dsh.rs` fallback retained in spec; G0 issue forces the conversation before code |
| Claude regression during port | Recorded-fixture streaming comparison + full existing suite as G1 gate |
| Manifest breakage for existing users | Absent-provider default + deprecation note; golden manifest tests |
| Scope creep into codex/aider executors | Explicitly out of scope; registry makes them cheap follow-ups |

## 6. Draft upstream issue (ready to post)

> **[proposal] Generalize task execution: ProviderExecutor trait + provider registry**
>
> Task execution is currently Claude-coupled: `__claude_task__` string dispatch,
> `claude.rs` module, `claude:` manifest block. Adding a second task-capable
> platform (we maintain an `agentic/dsh` image — DeepSeek Harness CLI, PR-able
> separately) would clone all three.
>
> Proposal: a `TaskExecutor` trait with a static provider registry and a generic
> `TaskConfig` (provider-tagged; absent provider defaults to claude for backward
> compat). Runner-level invariants become single-sourced: workload-identity
> gating before spawn, credential-lease→env resolution per provider mapping, and
> the unified error taxonomy. `RenewCertificate` lifecycle is transport-level and
> unaffected.
>
> Happy to bring this as: (1) behavior-preserving claude.rs refactor, then
> (2) `dsh` as the second registrant proving the trait, then (3) docs. Registered
> model provider for dsh is OpenRouter via the standard credential-lease path
> (`AGENTIC_CREDENTIAL_DIR`), not a native vendor subscription — the lease→env
> mapping is per-provider and follows `anthropic_api_key`/`openai_api_key`
> conventions (`openrouter_api_key`).
>
> Related: #175, #181 (parity epic), #266 (pin policy).

## 7. Post-merge ops

AIWG capability-matrix row (`deepseek-harness: Tier 1, tasks: native`), steward
routing note, getting-started platform table entry, and the five-project fleet
migrated to manifest-driven tasks (no pane-driving required for routine work).

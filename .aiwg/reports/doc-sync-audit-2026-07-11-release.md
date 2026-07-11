# Doc-Sync Audit — v2026.7.6 release

**Date:** 2026-07-11
**Direction:** code-to-docs
**Scope:** management, agent-rs, cli, scripts, images/qemu, docs, .aiwg
**Method:** authored-inline + deterministic checks (this release's surface is
small and additive; docs were written alongside the code, so a full multi-agent
doc-sync pass was not required).

## Changed code surface this release

| Change | Doc updated |
|---|---|
| `GET /api/v1/agent-output/chat` (#600) | `docs/API.md` (endpoint section), `docs/contracts/extensions/agent-output/v1/spec.md` |
| AgentCard `agent-output/v1` extension (#630) | `docs/contracts/extensions/agent-output/v1/spec.md`; UI `EXT_DOC_LINKS` doc-link |
| Session `chat_source` / `chat_stream_url` (#600/#629) | `docs/API.md` (chat section notes capability advertisement) |
| Admin dashboard panels + reprovision control (#628/#629/#631) | operator-facing; `CHANGELOG.md`, `docs/releases/v2026.7.6.md` |

## Checks

- `scripts/check-doc-links.py` → **passed** (no broken links in `docs/`,
  including the new `agent-output/v1` spec and v2026.7.6 release notes).
- `CHANGELOG.md` → `## [2026.7.6]` section present and populated.
- `docs/releases/v2026.7.6.md` → present.
- New contract doc cross-links (`pty-extensions/v1`, Fortemi) resolve.

## Result

No documentation drift for the released code surface. All new/changed public
surfaces are documented. Gate satisfied.

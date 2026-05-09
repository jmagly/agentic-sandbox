# Documentation Promotion Plan

**Date**: 2026-05-09
**Status**: Draft
**Owner**: agentic-sandbox / roctinam
**Linked**: gap analysis from issue-planner kickoff session

## Purpose

Resolve the documentation gap analysis findings by **promoting existing internal SDLC artifacts to public documentation**, **filling genuinely missing content**, and **cleaning up the docsite manifest**. Decouples the doc work from the v2 executor contract initiative so it can ship in parallel.

## Findings recap

The earlier gap analysis identified 36 broken intra-doc links (now fixed) and called out major content gaps:

- "Planned" docs referenced from `docs/reliability-README.md`, `docs/reliability-quickstart.md`, `docs/LIFECYCLE.md`, `docs/observability/README.md`.
- No `docs/container-runtime.md` despite container parity being the headline 2026.5.0 feature.
- No standalone PTY/screen-state, crash-loop, telemetry pipeline, or transport-audit docs.
- No `agent-rs/`, `cli/`, `proto/`, `deploy/`, `sdk/`, `runtimes/` README files.
- No Platform Support Matrix doc despite the multi-distro/Proxmox epic (#114/115/118/119/120).
- Welcome page links only old top-level docs; misses container runtime, AIWG bridge, dashboard, loadouts.
- `research/` content ships in the public docsite even though it's decision-support, not user-facing.

**Critical finding from this planning round**: many of the "planned" docs **already exist in `.aiwg/architecture/`** as internal SDLC artifacts but were never promoted to `docs/`:

| Already exists in `.aiwg/architecture/` | Mapped to "planned" reference in docs/ |
|---|---|
| `OBSERVABILITY_DESIGN.md` | `docs/observability/README.md` link |
| `SESSION_ARCHITECTURE.md` | `docs/LIFECYCLE.md` link (currently repointed to SESSION_RECONCILIATION) |
| `reliability-design.md` | `docs/reliability-README.md` link |
| `reliability-architecture.md` | `docs/reliability-README.md` link |
| `reliability-design-summary.md` | `docs/reliability-README.md` link |
| `reliability-implementation-checklist.md` | `docs/reliability-README.md` link |
| `runtime-parity.md` | (no current doc reference; covers container parity) |
| `management-server-design.md` | (no current doc reference) |
| `grpc-architecture.md` | (no current doc reference) |
| `VM_LOADOUT.md` | possibly redundant with `docs/LOADOUTS.md` |

This is a **promotion problem**, not an authoring problem. The docs exist; they just live in the internal SDLC corpus.

## Strategy

Three streams of work, executable in parallel:

### Stream A: Promote existing SDLC artifacts

For each artifact in `.aiwg/architecture/` listed above:

1. Review for currentness vs. code reality.
2. If still accurate: copy to `docs/` (or the appropriate subdirectory) with light front-matter cleanup.
3. If partially stale: update inline or link to a TODO callout.
4. Update `docs/_manifest.json` to include the promoted file in the navigation order.
5. Replace the `*(planned)*` placeholder text in linking docs with real `[link](file.md)`.

### Stream B: Author missing content

For documentation that doesn't exist anywhere yet:

1. **`docs/container-runtime.md`** — `docker_runtime.rs` reference; image catalog; container vs VM trade-offs; AIWG bridge integration.
2. **`docs/pty-rendering.md`** — design rationale for the formal session protocol; multi-controller semantics; replay buffer; PTY corruption history (#180).
3. **`docs/crash-loop.md`** — `crash_loop.rs` semantics; configuration; operator-visible behavior.
4. **`docs/telemetry.md`** — agent-side metrics → mgmt-side aggregation; Prometheus surface; observability backplane integration.
5. **`docs/transport-audit.md`** — `/api/v1/logs` ring buffer + `/api/v1/events` SSE; what gets logged, retention, format.
6. **`docs/platform-support.md`** — supported VMs (Ubuntu agentic-dev, Alpine target), supported runtimes (libvirt, future Proxmox), agent-rs build targets (glibc, musl).
7. **READMEs**: `agent-rs/README.md`, `cli/README.md`, `proto/README.md`, `deploy/README.md`. (`sdk/`, `runtimes/` audited for relevance first.)

### Stream C: Docsite hygiene

1. Update `docs/welcome.md` quick-links to include container runtime, AIWG bridge, dashboard, loadouts, monitoring, CHANGELOG.
2. Add `docs/glossary.md` with terms: agentshare, loadout, mission, executor, HITL, instance, runtime, session, dispatch, etc.
3. Add `docs/concepts.md` (mental model: sessions vs tasks vs runs vs missions vs agents vs instances).
4. Exclude `docs/research/` from `_manifest.json` (or move to a separate "Research notes" section with explicit framing).
5. Audit `docs/architecture/recommended-design.md` — orphaned; either link or remove.
6. Audit orphan code dirs: `api/`, `runtimes/qemu/`, top-level `agent/` (Python gRPC client predates `agent-rs/`), `sdk/python/`. For each, decide: integrate, document as legacy, or delete. File issues for the cleanup work.

## Sequence

| Wave | Work | Why this order |
|---|---|---|
| **Wave 1** | Stream A (promote existing artifacts) | Highest leverage; resolves dead-link placeholders quickly; no authoring effort |
| **Wave 2** | Stream B items 1, 2 (container runtime + PTY) | Major v2026.5.0 features with zero current coverage |
| **Wave 3** | Stream C (welcome refresh, glossary, concepts) | Improves discoverability of newly-promoted content |
| **Wave 4** | Stream B items 3–6 + READMEs | Lower-priority subsystem docs |
| **Wave 5** | Orphan-dir audit | Codebase question; doc work depends on outcome |

Waves 1, 2, 3 can be done in parallel by one engineer in ~3 focused sessions. Waves 4, 5 are filler / longer-tail.

## Success criteria

- S1: Zero `*(planned)*` markers left in `docs/` after Wave 1.
- S2: `_manifest.json` order list contains the promoted artifacts; nav reflects the full doc set.
- S3: `docs/welcome.md` quick-links cover at least: ARCHITECTURE, DEPLOYMENT, OPERATIONS, API, TROUBLESHOOTING, container-runtime, aiwg-executor, LOADOUTS, monitoring, CHANGELOG.
- S4: `docs/glossary.md` and `docs/concepts.md` exist; both linked from welcome.
- S5: `agent-rs/`, `cli/`, `proto/`, `deploy/` each have a README ≥30 lines.
- S6: `docs/research/` is either excluded from the docsite manifest or moved into a separate "Research notes" section header.
- S7: Build harness reports zero broken links with `strictLinks: true` (already true; preserved).

## Anti-goals

- Do NOT recreate content that already exists in `.aiwg/`. Promote, don't duplicate.
- Do NOT block on the v2 executor contract initiative. This work ships independently.
- Do NOT expand scope: orphan-dir audit is bounded to deciding "keep, document as legacy, or delete" — actual code cleanup is a separate task.

## Open questions

- Q1: Should `.aiwg/architecture/` artifacts be moved to `docs/` (deleting the .aiwg copy) or copied (.aiwg keeps internal version)? Recommendation: move; .aiwg/ is for in-flight artifacts, `docs/` is the published surface. (Risk: future SDLC work treats .aiwg/ as canonical.)
- Q2: Should the existing SDLC corpus (vision-document.md, ADRs, use-case briefs) be promoted to `docs/sdlc/`? Recommendation: no — those are internal artifacts; expose only what end-users need.
- Q3: `runtime-parity.md` and `management-server-design.md` — promote to top-level docs/, or fold into ARCHITECTURE.md? Recommendation: promote as separate docs (they're focused enough); link from ARCHITECTURE.md.

## References

- Earlier gap analysis (this session)
- `docs/_manifest.json` (current navigation)
- `docs/welcome.md` (current welcome)
- `.aiwg/architecture/` (existing SDLC artifacts to promote)

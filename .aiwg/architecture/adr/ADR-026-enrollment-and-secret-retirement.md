# ADR-026: Zero-Touch Enrollment; Retire Shared Secret and TOFU

## Status

Accepted (2026-05-31; Phase 3 cutover trigger selected in
`agentic-sandbox#408`)

## Date

2026-05-31

## Context

Today a new agent is "enrolled" by baking a long-lived `AGENT_SECRET` in
plaintext into the cloud-init ISO `[INT-6]`, and unknown ids are
**auto-registered on first connect** (TOFU) `[INT-4]`. Both violate project
rules (`token-security` `[RULE-3]`, `sec-key-material-handling` `[RULE-2]`) and
are the sharpest spoofing surface in the threat model (§2 Spoofing). Enrollment
must become zero-touch (G-3) while closing these holes (G-4).

## Decision

### Local build — implicit (host-mediated) enrollment, no token
Management **creates** the container/VM and owns the UDS/CID, so it already
knows the agent's identity at birth. There is **nothing to enroll**: identity
is established by the transport itself (ADR-023/024). No secret, no token, no
cert on UDS/vsock.

### Fleet build — in-VM keygen + single-use bootstrap token
1. Agent generates its keypair **in-VM**; the private key never leaves it
   `[RULE-2]`.
2. Provisioning injects a **single-use, short-TTL** bootstrap token + the CA
   trust bundle (public) via cloud-init.
3. Agent CSRs to the backend presenting the token; backend validates the token
   (one-time) and the requested SPIFFE id, signs a short-lived leaf
   `[STD-SPIFFE] [TOOL-STEPCA] [TOOL-VAULT]`.
4. Token is consumed; subsequent identity is the cert.

This mirrors established node-enrollment patterns (step-ca OTT, k8s
bootstrap-token + CSR approval, SPIRE join token) `[STD-SPIRE]` (GRADE
MODERATE/LOW — confirm specifics, R-9).

### Retire shared secret and TOFU
- Remove generation/injection of `AGENT_SECRET` from provisioning `[INT-6]`.
- Remove the TOFU auto-register branch in `SecretStore::verify` `[INT-4]`;
  unknown identity ⇒ **reject** (FR-7, AC-7).
- Keep `SecretStore` only behind the dual-mode compat flag during migration
  (ADR-023 §config, FR-8), deleted at cutover.
- Phase 3 cutover trigger: remove legacy secret/TOFU only after the default
  agent image fleet ships the transport-aware client and the Phase 2
  `mode=auto` path has passed the integration/capture gates for the released
  image cohort. Until then `accept_legacy_secret=true` remains a migration
  valve for existing agents.

### "Inject key+cert at provision" vs "in-VM keygen + CSR"
| Approach | Key custody | Verdict |
|----------|-------------|---------|
| Mint key+cert at provision, inject via ISO | private key transits the ISO (like the secret today) | weaker — rejected as default |
| **In-VM keygen + single-use-token CSR (chosen)** | key never leaves the agent | preferred for fleet |
| **Implicit (host-mediated), no key at all (chosen, local)** | nothing to transit | best — local default |

## Consequences

### Positive
- Local enrollment is literally zero-step (S-2, S-4).
- Eliminates the plaintext long-lived credential and TOFU (G-4, closes the
  top Spoofing/Disclosure threats).
- Bootstrap-token blast radius bounded to a single enrollment (R-3).

### Negative
- **Sequencing risk (R-6)**: TOFU/secret removal must come *after* the
  dual-mode window proves the new path, or in-flight agents break. Sequenced in
  the rollout plan.
- Fleet token distribution still needs the provisioning channel to be at least
  integrity-protected; documented as a fleet-build assumption.

## Alternatives Considered
Keep TOFU with an operator approval gate (still a manual step + spoofable
window) — rejected against G-3/G-4.

## References
- @.aiwg/architecture/adr/ADR-023-transport-per-runtime-security.md
- @.aiwg/planning/agent-transport-security-rollout.md
- @.aiwg/security/agent-transport-security-references.md

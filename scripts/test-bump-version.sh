#!/usr/bin/env bash
# test-bump-version.sh — smoke test for scripts/bump-version.sh
#
# Exercises the CHANGELOG-editing path of bump-version.sh against a fresh
# fixture and asserts the four acceptance criteria from issue #315:
#
#   1. New compare-link lands in the footer block (lines of the form
#      `[X.Y.Z]: https://...` at the bottom of the file).
#   2. No compare-link lines outside the footer block.
#   3. `[Unreleased]:` rewritten to the canonical full URL — `https://...compare/vNEW...HEAD`,
#      with leading `v`. The pre-fix bug introduced `P26.5.4...HEAD` etc.
#   4. New `## [NEW] — DATE` section inserted under `## [Unreleased]`.
#
# Run from repo root:  bash scripts/test-bump-version.sh
# Exit code: 0 = all asserts pass, 1 = something failed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$REPO_ROOT/scripts/bump-version.sh"
[ -x "$SCRIPT" ] || { echo "fatal: $SCRIPT not executable" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
# Use $((x + 1)) rather than ((x++)) — under `set -e`, the post-increment
# form exits non-zero when the pre-increment value is 0 (C-style return
# semantics), which falsely fails the &&/|| chain on the first pass call.
pass() { echo "  ✓ $1"; PASS=$((PASS + 1)); }
fail() { echo "  ✗ $1" >&2; FAIL=$((FAIL + 1)); }

# ----------------------------------------------------------------------------
# fixture
# ----------------------------------------------------------------------------
#
# CHANGELOG fixture deliberately includes the pathological case from issue
# #315: a body bullet that literally mentions the text `[Unreleased]:` in
# backticks. The pre-fix regex matched THIS string and inserted the new
# compare-link mid-section.

cat > "$TMP/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

_Nothing yet._

## [2026.5.4] — 2026-05-20

### Fixed

- **Broken `[Unreleased]:` compare link** (this commit): the footer link
  `[Unreleased]: P26.5.3...HEAD` was malformed. Fixed inline.

## [2026.5.3] — 2026-05-19

### Added

- Initial release.

[Unreleased]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.4...HEAD
[2026.5.4]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.3...v2026.5.4
[2026.5.3]: https://git.integrolabs.net/roctinam/agentic-sandbox/releases/tag/v2026.5.3
EOF

# bump-version.sh expects to be inside a git repo with the matching Cargo
# files. Stub them out in TMP, init a git repo, then run the script with
# the python CHANGELOG-edit block redirected to operate on the fixture.

cd "$TMP"
git init -q
mkdir -p management agent-rs cli scripts
for d in management agent-rs cli; do
  cat > "$d/Cargo.toml" <<C
[package]
name = "stub"
version = "2026.5.4"
edition = "2021"
C
  cat > "$d/Cargo.lock" <<C
# fake lock
[[package]]
name = "stub"
version = "2026.5.4"

[[package]]
name = "agentic-management"
version = "2026.5.4"

[[package]]
name = "agent-client"
version = "2026.5.4"

[[package]]
name = "agentic-cli"
version = "2026.5.4"
C
done
# Copy script in before the commit so the working tree is clean when
# bump-version.sh's dirty-tree guard runs.
cp "$SCRIPT" "$TMP/scripts/bump-version.sh"
git add -A && git -c user.email=test@test.local -c user.name=test commit -q -m init

# Bump-output log lives outside the working tree so it doesn't dirty it.
BUMP_LOG="${TMP}.bump.log"
if bash scripts/bump-version.sh 2026.5.5 > "$BUMP_LOG" 2>&1; then
  pass "bump exits 0"
else
  fail "bump exits non-zero — log follows"
  sed 's/^/    /' < "$BUMP_LOG" >&2
fi

# ----------------------------------------------------------------------------
# asserts
# ----------------------------------------------------------------------------

CL="$TMP/CHANGELOG.md"

# (1) New compare-link lands in the footer block.
# The footer is the contiguous run of `[*]: https://...` lines at the bottom.
# Last 10 lines must contain the new compare-link.
if tail -10 "$CL" | grep -qE '^\[2026\.5\.5\]: https://git\.integrolabs\.net/[^/]+/agentic-sandbox/compare/v2026\.5\.4\.\.\.v2026\.5\.5$'; then
  pass "new compare-link is in the footer block"
else
  fail "new compare-link is NOT in the footer block (last 10 lines):"
  tail -10 "$CL" | sed 's/^/    /'
fi

# (2) No compare-links outside the footer.
# Find every `^[*]: https://...` line; assert they are all in the last 10 lines.
TOTAL_LINKS=$(grep -cE '^\[[^]]+\]: https://' "$CL" || true)
FOOTER_LINKS=$(tail -10 "$CL" | grep -cE '^\[[^]]+\]: https://' || true)
if [ "$TOTAL_LINKS" = "$FOOTER_LINKS" ]; then
  pass "all $TOTAL_LINKS compare-links are in the footer (none mis-inserted into section bodies)"
else
  fail "found $TOTAL_LINKS compare-links total but only $FOOTER_LINKS in footer — $((TOTAL_LINKS - FOOTER_LINKS)) mis-inserted"
  grep -nE '^\[[^]]+\]: https://' "$CL" | sed 's/^/    /'
fi

# (3) [Unreleased] link is canonical full URL with leading `v`.
UNREL_LINE=$(grep -E '^\[Unreleased\]:' "$CL" | head -1)
EXPECTED="[Unreleased]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v2026.5.5...HEAD"
if [ "$UNREL_LINE" = "$EXPECTED" ]; then
  pass "[Unreleased] link is canonical full URL"
else
  fail "[Unreleased] link is wrong"
  echo "    expected: $EXPECTED" >&2
  echo "    actual:   $UNREL_LINE" >&2
fi

# (4) New section header inserted under [Unreleased].
if grep -qE '^## \[2026\.5\.5\] — [0-9]{4}-[0-9]{2}-[0-9]{2}' "$CL"; then
  pass "new section header [2026.5.5] inserted"
else
  fail "new section header [2026.5.5] NOT inserted"
fi

# Extra guard: bullet that literally quotes `[Unreleased]:` in backticks
# must not have a compare-link line bolted on after it.
if grep -B1 -A1 '`\[Unreleased\]:' "$CL" | grep -qE '^\[2026\.5\.5\]:'; then
  fail "compare-link bolted on next to the prose mention of [Unreleased]: in backticks (bug 1 not fixed)"
else
  pass "no compare-link wedged into prose mentioning [Unreleased]:"
fi

# ----------------------------------------------------------------------------
# summary
# ----------------------------------------------------------------------------

echo ""
echo "─────────────────────────────────────────"
echo "  ${PASS} passed, ${FAIL} failed"
echo "─────────────────────────────────────────"

if [ "$FAIL" -gt 0 ]; then
  echo ""
  echo "Resulting CHANGELOG.md (for inspection):"
  cat "$CL" | sed 's/^/    /'
  exit 1
fi
exit 0

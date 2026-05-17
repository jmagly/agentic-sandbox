#!/usr/bin/env bash
# lint-ci-pins.sh — fail on unpinned `uses:` or floating `container:` references
# in .gitea/workflows/.
#
# Policy (issue #261):
#   - Every `uses:` MUST reference a 40-char commit SHA, not a tag or branch
#   - Every `container:`/`image:` MUST be `<name>:<tag>@sha256:<digest>`
#   - Every pin MUST appear in ci/digests.txt
#
# Exits 0 if clean, 1 otherwise.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail=0
findings=()

# Pattern: `uses: <owner>/<repo>@<ref>` where <ref> is NOT a 40-char hex SHA.
while IFS= read -r line; do
  [ -z "$line" ] && continue
  file="${line%%:*}"
  rest="${line#*:}"
  lineno="${rest%%:*}"
  content="${rest#*:}"

  # Strip inline comment so trailing `# v4.3.1` doesn't confuse the SHA check
  ref_part="$(echo "$content" | sed -E 's/[[:space:]]+#.*$//')"

  # Extract the @<ref> portion
  ref="$(echo "$ref_part" | sed -nE 's/.*uses:[[:space:]]*[a-zA-Z0-9._/-]+@([^[:space:]]+).*/\1/p')"

  [ -z "$ref" ] && continue

  # Skip local workflow references (./ or just a relative path)
  case "$ref" in ./*|/*) continue ;; esac

  # SHA must be 40 hex chars
  if ! echo "$ref" | grep -qE '^[0-9a-f]{40}$'; then
    findings+=("$file:$lineno: unpinned uses ref '$ref' — not a 40-char SHA: $content")
    fail=1
  fi
done < <(grep -rnE '^[[:space:]]*-?[[:space:]]*uses:[[:space:]]*[a-zA-Z0-9._/-]+@' .gitea/workflows/ 2>/dev/null)

# Pattern: `container: <image>:<tag>` without `@sha256:`
# Also catches `image: ...` at the same indent level.
while IFS= read -r line; do
  [ -z "$line" ] && continue
  file="${line%%:*}"
  rest="${line#*:}"
  lineno="${rest%%:*}"
  content="${rest#*:}"

  # Strip inline comment
  body="$(echo "$content" | sed -E 's/[[:space:]]+#.*$//')"
  # Extract image value
  img="$(echo "$body" | sed -nE 's/.*(container|image):[[:space:]]*([^[:space:]]+).*/\2/p')"
  [ -z "$img" ] && continue

  # Skip if already digest-pinned
  if echo "$img" | grep -q '@sha256:'; then
    continue
  fi
  findings+=("$file:$lineno: unpinned container/image '$img' — no @sha256 digest: $content")
  fail=1
done < <(grep -rnE '^[[:space:]]+(container|image):[[:space:]]+[^[:space:]#]+' .gitea/workflows/ 2>/dev/null)

if [ $fail -eq 0 ]; then
  echo "✓ lint-ci-pins: all uses: and container: references are pinned"
  exit 0
fi

echo "✗ lint-ci-pins: policy violations found"
echo
for f in "${findings[@]}"; do
  echo "  $f"
done
echo
echo "Policy: every \`uses:\` must reference a 40-char SHA; every \`container:\`/\`image:\` must include @sha256:<digest>."
echo "Add the pin to ci/digests.txt and reference it."
echo "See: .claude/rules/ci-action-pinning.md, issue #261"
exit 1

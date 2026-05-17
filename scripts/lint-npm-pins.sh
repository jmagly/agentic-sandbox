#!/usr/bin/env bash
# lint-npm-pins.sh — fail on any unpinned `npm install -g` invocation.
#
# Policy (issue #266):
#   - Every `npm install -g <pkg>` MUST pin a concrete version: `<pkg>@<version>`
#   - Moving tags (@latest, @next) are forbidden — semantically equivalent to :latest
#   - Each invocation SHOULD pass `--ignore-scripts` unless lifecycle scripts are required
#   - Every pinned package MUST appear in ci/npm-pins.txt
#
# Exits 0 if clean, 1 otherwise. Designed to be wired into the schema-lint CI job.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail=0
findings=()

# Search paths: CI, container images, qemu provisioning. Skip .backup files.
search_paths=(.gitea images scripts)

# Grep for npm install -g / npm i -g across the search paths
while IFS= read -r line; do
  [ -z "$line" ] && continue
  file="${line%%:*}"
  rest="${line#*:}"
  lineno="${rest%%:*}"
  content="${rest#*:}"

  # Skip backup files and this script itself
  case "$file" in
    *.backup|*.backup-*|scripts/lint-npm-pins.sh) continue ;;
    ci/npm-pins.txt) continue ;;
  esac

  # Skip comments
  trimmed="${content#"${content%%[![:space:]]*}"}"
  case "$trimmed" in '#'*|'//'*) continue ;; esac

  # Strip inline shell/YAML comments so we don't false-positive on documentation
  # like `- string  # npm install -g targets`. Shell # must be preceded by space.
  content="$(echo "$content" | sed -E 's/[[:space:]]+#.*$//')"
  trimmed="${content#"${content%%[![:space:]]*}"}"
  # Skip the line entirely if the npm reference was inside the stripped comment
  case "$trimmed" in *npm*install*-g*|*npm*i\ -g*) ;; *) continue ;; esac

  # Check for moving tags
  if echo "$content" | grep -qE '@(latest|next|canary|nightly)([[:space:]]|$|"|'"'"')'; then
    findings+=("$file:$lineno: moving tag (forbidden): $trimmed")
    fail=1
    continue
  fi

  # Extract the package arguments after `npm install -g` / `npm i -g`.
  # Stop at any shell control operator or redirection — anything after a `|`,
  # `&&`, `||`, `;`, `>`, `<`, `2>`, or backslash-quote is not an npm package.
  pkg_part=$(echo "$content" | sed -E 's#^.*npm (install|i) -g##')
  # Truncate at first control operator
  pkg_part=$(echo "$pkg_part" | sed -E 's#( \|\||  *\||  *&&|  *;|  *>|  *<|  *2>| 2>&1).*$##')

  for tok in $pkg_part; do
    # Skip flags
    case "$tok" in --*|-?) continue ;; esac
    # Skip line-continuation backslashes
    case "$tok" in '\\') continue ;; esac
    # Must look like a package: @scope/name or name (no operators, no quotes)
    if echo "$tok" | grep -qE '^(@[a-z0-9._-]+/[a-z0-9._-]+|[a-z0-9._-]+)$'; then
      # Unpinned bare package name
      findings+=("$file:$lineno: unpinned package '$tok': $trimmed")
      fail=1
    fi
  done
done < <(grep -rnE 'npm (install|i) -g' "${search_paths[@]}" 2>/dev/null || true)

if [ $fail -eq 0 ]; then
  echo "✓ lint-npm-pins: all npm install -g invocations are version-pinned"
  exit 0
fi

echo "✗ lint-npm-pins: policy violations found"
echo
for f in "${findings[@]}"; do
  echo "  $f"
done
echo
echo "Policy: every \`npm install -g <pkg>\` must use <pkg>@<version>."
echo "Floating tags (@latest, @next) are forbidden."
echo "Add or update the pin in ci/npm-pins.txt and reference it."
echo "See: .claude/rules/dependency-source-policy.md, issue #266"
exit 1

#!/usr/bin/env bash
# bump-version.sh — single-shot version bump across all release manifests.
#
# Replaces the manual edit dance documented in docs/releases/runbook.md.
# Updates:
#   - management/Cargo.toml, agent-rs/Cargo.toml, cli/Cargo.toml
#   - their matching Cargo.lock entries
#   - CHANGELOG.md: inserts a new `## [<version>] — <today>` section
#     under Unreleased with placeholder body
#   - CHANGELOG.md: updates the compare-link footer
#
# Usage:
#   scripts/bump-version.sh 2026.5.3
#   scripts/bump-version.sh 2026.5.3 --date 2026-05-19
#   scripts/bump-version.sh 2026.5.3 --dry-run
#
# Exit codes:
#   0 — success
#   1 — usage / version-format error
#   2 — working tree dirty
#   3 — version already present in CHANGELOG
#   4 — file edit failed
#
# Per .claude/rules/versioning.md: NO leading zeros in any version component.

set -euo pipefail

# -----------------------------------------------------------------------------
# argument parsing
# -----------------------------------------------------------------------------

NEW_VERSION=""
TODAY="$(date -u '+%Y-%m-%d')"
DRY_RUN=0

usage() {
  cat <<USAGE
usage: $(basename "$0") <new-version> [--date YYYY-MM-DD] [--dry-run]

  <new-version>     CalVer YYYY.M.PATCH (no leading zeros, e.g. 2026.5.3)
  --date            override the date stamped into the CHANGELOG heading
  --dry-run         print intended changes without modifying any files

example:
  $(basename "$0") 2026.5.3
USAGE
  exit 1
}

[ $# -ge 1 ] || usage
case "$1" in -h|--help) usage ;; esac

NEW_VERSION="$1"
shift
while [ $# -gt 0 ]; do
  case "$1" in
    --date)    TODAY="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage ;;
    *) echo "unknown argument: $1" >&2; usage ;;
  esac
done

# -----------------------------------------------------------------------------
# validation
# -----------------------------------------------------------------------------

# CalVer YYYY.M.PATCH, no leading zeros anywhere.
if ! [[ "$NEW_VERSION" =~ ^[0-9]{4}\.([1-9]|1[0-2])\.(0|[1-9][0-9]*)$ ]]; then
  echo "error: version '$NEW_VERSION' is not valid CalVer YYYY.M.PATCH" >&2
  echo "       (no leading zeros: '2026.5.3' OK; '2026.05.3' BAD)" >&2
  exit 1
fi

if ! [[ "$TODAY" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  echo "error: date '$TODAY' is not YYYY-MM-DD" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$REPO_ROOT" ]; then
  echo "error: not inside a git repository" >&2
  exit 1
fi
cd "$REPO_ROOT"

if [ "$DRY_RUN" -eq 0 ] && [ -n "$(git status --porcelain)" ]; then
  echo "error: working tree is dirty; commit or stash before bumping version" >&2
  git status --short >&2
  exit 2
fi

if grep -qE "^## \[${NEW_VERSION//./\\.}\]" CHANGELOG.md; then
  echo "error: CHANGELOG.md already has a '## [${NEW_VERSION}]' section" >&2
  exit 3
fi

# Discover current version from management/Cargo.toml (canonical).
CURRENT_VERSION=$(grep -m1 '^version = ' management/Cargo.toml | cut -d'"' -f2)
[ -n "$CURRENT_VERSION" ] || { echo "error: cannot read current version from management/Cargo.toml" >&2; exit 4; }

if [ "$CURRENT_VERSION" = "$NEW_VERSION" ]; then
  echo "error: current version is already $NEW_VERSION; nothing to bump" >&2
  exit 1
fi

# -----------------------------------------------------------------------------
# plan
# -----------------------------------------------------------------------------

CARGO_TOMLS=(management/Cargo.toml agent-rs/Cargo.toml cli/Cargo.toml)
CARGO_LOCKS=(management/Cargo.lock agent-rs/Cargo.lock cli/Cargo.lock)
LOCK_NAMES=("agentic-management" "agent-client" "agentic-cli")

echo "Version bump plan:"
echo "  ${CURRENT_VERSION}  →  ${NEW_VERSION}    (date: ${TODAY})"
echo "  Files to update:"
for f in "${CARGO_TOMLS[@]}" "${CARGO_LOCKS[@]}" CHANGELOG.md; do
  echo "    - $f"
done

if [ "$DRY_RUN" -eq 1 ]; then
  echo ""
  echo "[dry-run] no files modified"
  exit 0
fi

# -----------------------------------------------------------------------------
# apply
# -----------------------------------------------------------------------------

# Cargo.toml: replace the first `version = "<current>"` line in each.
for f in "${CARGO_TOMLS[@]}"; do
  if ! grep -q "^version = \"${CURRENT_VERSION}\"" "$f"; then
    echo "error: $f does not contain version = \"${CURRENT_VERSION}\"" >&2
    exit 4
  fi
  # sed -i with portable in-place edit. Match only the first occurrence to
  # avoid touching dependency `version = "..."` entries.
  awk -v old="${CURRENT_VERSION}" -v new="${NEW_VERSION}" '
    !done && /^version = "/ && $0 ~ ("\"" old "\"") {
      sub("\"" old "\"", "\"" new "\""); done=1
    }
    { print }
  ' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done

# Cargo.lock: update the version line directly after the matching package name.
for i in "${!CARGO_LOCKS[@]}"; do
  lock="${CARGO_LOCKS[$i]}"
  name="${LOCK_NAMES[$i]}"
  awk -v name="$name" -v old="${CURRENT_VERSION}" -v new="${NEW_VERSION}" '
    {
      print
      if ($0 == "name = \"" name "\"") {
        # next line should be: version = "<old>"
        if (getline line > 0) {
          sub("\"" old "\"", "\"" new "\"", line)
          print line
        }
      }
    }
  ' "$lock" > "$lock.tmp" && mv "$lock.tmp" "$lock"
done

# CHANGELOG.md: insert a new section under Unreleased, and update the
# compare-link footer. The skeleton intentionally omits Highlights/Removed
# so the release-prep author chooses categories per release.
python3 - "$NEW_VERSION" "$TODAY" "$CURRENT_VERSION" <<'PY'
import sys, re, pathlib

new_v, today, old_v = sys.argv[1], sys.argv[2], sys.argv[3]
path = pathlib.Path("CHANGELOG.md")
text = path.read_text()

new_section = f"""## [{new_v}] — {today}

_Populate before tagging. Suggested sections:_

### Added

### Changed

### Fixed

### Documentation

### Operator notes

"""

# Insert immediately after the `_Nothing yet._` line under [Unreleased].
# Fallback: insert after the [Unreleased] heading line if `_Nothing yet._`
# is absent (someone already edited Unreleased).
pat_nothing = re.compile(r"(## \[Unreleased\][^\n]*\n\n_Nothing yet\._\n)", re.MULTILINE)
m = pat_nothing.search(text)
if m:
    text = text.replace(m.group(1), m.group(1) + "\n" + new_section, 1)
else:
    pat_unrel = re.compile(r"(## \[Unreleased\][^\n]*\n\n)", re.MULTILINE)
    m = pat_unrel.search(text)
    if not m:
        sys.stderr.write("error: CHANGELOG.md has no [Unreleased] section\n")
        sys.exit(4)
    text = text.replace(m.group(1), m.group(1) + new_section, 1)

# Update the compare-link footer.
#
# The match must anchor to the footer's canonical URL form
# (`[Unreleased]: https://...compare/vOLDVER...HEAD`) at start-of-line so
# prose elsewhere in the CHANGELOG that happens to mention the literal
# `[Unreleased]:` (e.g. a bullet quoting the broken-link text) does not
# match. MULTILINE makes ^ anchor to start-of-line. See issue #315.
unrel_link_re = re.compile(
    r"^\[Unreleased\]: (https://[^\s]+/compare/v)"
    + re.escape(old_v)
    + r"\.\.\.HEAD",
    re.MULTILINE,
)
# Use \g<1> rather than \1 because new_v starts with digits and Python's
# re.sub parses \1<digits> as an octal escape (e.g. \12026 → \120 octal
# = 'P', producing the [Unreleased]: P26.5.5...HEAD typo seen in v2026.5.4
# and v2026.5.5 bumps). See issue #315 Bug 2.
new_unrel = unrel_link_re.sub(
    r"[Unreleased]: \g<1>" + new_v + r"...HEAD",
    text,
)

if new_unrel == text:
    sys.stderr.write(
        f"error: could not update [Unreleased] compare link for {old_v}\n"
    )
    sys.exit(4)
text = new_unrel

# Insert the new compare-link line right after the [Unreleased] footer link.
#
# Anchor to start-of-line + MULTILINE so the match is the literal footer
# line, never a prose mention of `[Unreleased]:` in a section body.
# See issue #315 Bug 1.
new_link = (
    f"[{new_v}]: https://git.integrolabs.net/roctinam/agentic-sandbox/compare/v{old_v}...v{new_v}\n"
)
insert_re = re.compile(
    r"(^\[Unreleased\]: https://[^\n]+\n)",
    re.MULTILINE,
)
new_text, n = insert_re.subn(
    r"\g<1>" + new_link,
    text,
    count=1,
)
if n == 0:
    sys.stderr.write(
        "error: could not locate canonical [Unreleased] footer link for insertion\n"
    )
    sys.exit(4)
text = new_text

path.write_text(text)
PY

# Sanity check: cargo can read the bumped versions.
#
# cargo pkgid emits one of two formats depending on workspace state:
#   - Old:  path+file:///abs/path#1.2.3
#   - New:  pkgname@1.2.3        (cargo ≥ 1.74; also when crate not in registry)
# Strip everything up to the last `#` or `@` to get the version.
# See issue #315 Bug 3 — the previous `s/.*#([^#]+)$/\1/` only handled the
# old format and produced false-positive `'pkgname@version'` warnings.
for crate_dir in management agent-rs cli; do
  v=$(cd "$crate_dir" && cargo pkgid 2>/dev/null | sed -E 's/.*[@#]//' || echo "")
  if [ -z "$v" ]; then
    echo "warn: cargo pkgid in $crate_dir returned nothing; check Cargo.lock manually" >&2
    continue
  fi
  if [ "$v" != "$NEW_VERSION" ]; then
    echo "warn: cargo pkgid in $crate_dir reports '$v', expected '$NEW_VERSION'" >&2
  fi
done

echo ""
echo "Version bumped: ${CURRENT_VERSION} → ${NEW_VERSION}"
echo ""
echo "Next steps:"
echo "  1. Populate the new [${NEW_VERSION}] section in CHANGELOG.md"
echo "  2. (optional) Create docs/releases/v${NEW_VERSION}.md from template"
echo "  3. git add -A && git commit -m 'chore(release): bump to ${NEW_VERSION}'"
echo "  4. git tag -a v${NEW_VERSION} -m '...'"
echo "  5. git push origin main && git push origin v${NEW_VERSION}"

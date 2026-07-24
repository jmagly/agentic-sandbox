#!/usr/bin/env bash
# Remove only files declared by an Agentic Sandbox macOS payload manifest.
set -euo pipefail

root="/"
manifest="/usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv"
confirm=0

usage() {
  cat <<'EOF'
Usage: uninstall-macos.sh [--root <path>] [--manifest <path>] [--confirm]

Removes only package-owned files and symlinks listed in the payload manifest.
It never unloads LaunchAgents or removes per-user state, workspaces, Keychain
items, TLS material, or user-rendered LaunchAgent files.

--confirm is required when --root is /.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --root) root="${2:-}"; shift 2 ;;
    --manifest) manifest="${2:-}"; shift 2 ;;
    --confirm) confirm=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[ -n "$root" ] && [ "${root#/}" != "$root" ] \
  || { echo "--root must be absolute" >&2; exit 2; }
[ -n "$manifest" ] && [ "${manifest#/}" != "$manifest" ] \
  || { echo "--manifest must be absolute" >&2; exit 2; }

if [ "$root" = "/" ] && [ "$confirm" != "1" ]; then
  echo "--confirm is required for a system-root uninstall" >&2
  exit 1
fi

case "$manifest" in
  "$root"/*) manifest_path="$manifest" ;;
  /*) manifest_path="${root%/}$manifest" ;;
esac
[ -f "$manifest_path" ] || { echo "payload manifest not found" >&2; exit 1; }

manifest_copy="$(mktemp -t agentic-macos-uninstall.XXXXXX)"
trap 'rm -f "$manifest_copy"' EXIT
install -m 0600 "$manifest_path" "$manifest_copy"

while IFS=$'\t' read -r entry_type _mode relative_path _value; do
  case "$relative_path" in
    ""|/*|*".."*) echo "unsafe payload manifest path" >&2; exit 1 ;;
  esac
  target="${root%/}/$relative_path"
  case "$entry_type" in
    file|symlink) rm -f "$target" ;;
    *) echo "unsupported payload manifest entry" >&2; exit 1 ;;
  esac
done < "$manifest_copy"

rm -f "$manifest_path"
for directory in \
  usr/local/share/doc/agentic-sandbox \
  usr/local/share/agentic-sandbox/launchd \
  usr/local/share/agentic-sandbox/config \
  usr/local/share/agentic-sandbox \
  usr/local/libexec/agentic-sandbox; do
  rmdir "${root%/}/$directory" 2>/dev/null || true
done

echo "Agentic Sandbox package-owned files removed; per-user state was preserved."

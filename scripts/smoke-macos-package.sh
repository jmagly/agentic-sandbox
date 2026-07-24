#!/usr/bin/env bash
# Credential-free isolated-root install/verify/uninstall smoke for a macOS pkg.
set -euo pipefail

package=""
install_root=""

usage() {
  cat <<'EOF'
Usage: smoke-macos-package.sh --package <pkg> --install-root <empty-dir>

Expands a package, copies its payload into an isolated root, validates every
manifest entry, and runs the packaged uninstaller against that root. It never
uses the system installer, loads launchd services, or mutates system paths.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --package) package="${2:-}"; shift 2 ;;
    --install-root) install_root="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[ -f "$package" ] || { echo "package not found" >&2; exit 1; }
[ -n "$install_root" ] && [ "${install_root#/}" != "$install_root" ] \
  || { echo "--install-root must be absolute" >&2; exit 2; }
command -v ditto >/dev/null 2>&1 || { echo "required command not found: ditto" >&2; exit 1; }
command -v pkgutil >/dev/null 2>&1 || { echo "required command not found: pkgutil" >&2; exit 1; }
command -v shasum >/dev/null 2>&1 || { echo "required command not found: shasum" >&2; exit 1; }

expanded="$(mktemp -d -t agentic-macos-expanded.XXXXXX)"
manifest_copy="$(mktemp -t agentic-macos-manifest.XXXXXX)"
trap 'rm -rf "$expanded"; rm -f "$manifest_copy"' EXIT

mkdir -p "$install_root"
[ -z "$(find "$install_root" -mindepth 1 -print -quit)" ] \
  || { echo "--install-root must be empty" >&2; exit 1; }

pkgutil --expand-full "$package" "$expanded"
manifest_found="$(find "$expanded" \
  -path '*/usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv' \
  -type f -print -quit)"
[ -n "$manifest_found" ] || { echo "expanded package payload manifest not found" >&2; exit 1; }
payload_root="${manifest_found%/usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv}"
install -m 0600 "$manifest_found" "$manifest_copy"
ditto "$payload_root" "$install_root"

while IFS=$'\t' read -r entry_type expected_mode relative_path expected_value; do
  target="${install_root%/}/$relative_path"
  case "$entry_type" in
    file)
      [ -f "$target" ] || { echo "installed payload file missing: $relative_path" >&2; exit 1; }
      actual_digest="$(shasum -a 256 "$target" | awk '{print $1}')"
      [ "$actual_digest" = "$expected_value" ] \
        || { echo "installed payload digest drift: $relative_path" >&2; exit 1; }
      actual_mode="$(stat -f '%Lp' "$target")"
      [ "$actual_mode" = "$expected_mode" ] \
        || { echo "installed payload mode drift: $relative_path" >&2; exit 1; }
      ;;
    symlink)
      [ -L "$target" ] || { echo "installed payload symlink missing: $relative_path" >&2; exit 1; }
      [ "$(readlink "$target")" = "$expected_value" ] \
        || { echo "installed payload symlink drift: $relative_path" >&2; exit 1; }
      ;;
    *) echo "unsupported payload manifest entry" >&2; exit 1 ;;
  esac
done < "$manifest_copy"

"$install_root/usr/local/libexec/agentic-sandbox/uninstall-macos" \
  --root "$install_root" \
  --manifest /usr/local/share/doc/agentic-sandbox/PAYLOAD-MANIFEST.tsv

while IFS=$'\t' read -r _entry_type _expected_mode relative_path _expected_value; do
  [ ! -e "${install_root%/}/$relative_path" ] \
    || { echo "package-owned path remained after uninstall: $relative_path" >&2; exit 1; }
done < "$manifest_copy"

echo "macOS isolated package install/uninstall smoke: pass"

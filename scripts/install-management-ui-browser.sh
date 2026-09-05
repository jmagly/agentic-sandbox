#!/usr/bin/env bash
# Install the pinned Chrome for Testing headless shell used by the management
# dashboard's browser smoke gate when the runner has no system Chrome.

set -euo pipefail

VERSION="152.0.7977.82"
ARCHIVE_SHA256="0ca12ea26b502a83e32db334a17883c315348845efc071d908de7a6d94a97eff"
PLATFORM="linux64"
ARCHIVE_NAME="chrome-headless-shell-${PLATFORM}.zip"
DOWNLOAD_URL="https://storage.googleapis.com/chrome-for-testing-public/${VERSION}/${PLATFORM}/${ARCHIVE_NAME}"

case "$(uname -m)" in
    x86_64|amd64) ;;
    *)
        echo "error: the pinned management UI browser supports Linux x86_64 runners only" >&2
        exit 2
        ;;
esac

CACHE_ROOT="${MANAGEMENT_UI_BROWSER_CACHE_DIR:-${RUNNER_TEMP:-/tmp}/agentic-sandbox-browser}"
INSTALL_ROOT="${CACHE_ROOT}/${VERSION}"
BROWSER_BIN="${INSTALL_ROOT}/chrome-headless-shell-${PLATFORM}/chrome-headless-shell"
if [[ -x "$BROWSER_BIN" ]]; then
    printf '%s\n' "$BROWSER_BIN"
    exit 0
fi

mkdir -p "$CACHE_ROOT"
ARCHIVE="${MANAGEMENT_UI_BROWSER_ARCHIVE:-${CACHE_ROOT}/${ARCHIVE_NAME}}"
DOWNLOADED_ARCHIVE=0
if [[ ! -f "$ARCHIVE" ]]; then
    curl --fail --location --proto '=https' --tlsv1.2 --retry 3 \
        --output "${ARCHIVE}.partial" "$DOWNLOAD_URL"
    mv "${ARCHIVE}.partial" "$ARCHIVE"
    DOWNLOADED_ARCHIVE=1
fi

ACTUAL_SHA256="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
if [[ "$ACTUAL_SHA256" != "$ARCHIVE_SHA256" ]]; then
    if (( DOWNLOADED_ARCHIVE )); then rm -f -- "$ARCHIVE"; fi
    echo "error: pinned management UI browser checksum mismatch" >&2
    exit 1
fi

STAGE="$(mktemp -d "${CACHE_ROOT}/extract.XXXXXX")"
cleanup() {
    rm -rf -- "$STAGE"
}
trap cleanup EXIT INT TERM
python3 - "$ARCHIVE" "$STAGE" <<'PY'
import pathlib
import sys
import zipfile

archive = pathlib.Path(sys.argv[1])
destination = pathlib.Path(sys.argv[2]).resolve()
with zipfile.ZipFile(archive) as bundle:
    for member in bundle.infolist():
        target = (destination / member.filename).resolve()
        if destination not in target.parents and target != destination:
            raise SystemExit(f"unsafe browser archive path: {member.filename}")
    bundle.extractall(destination)
PY

rm -rf -- "$INSTALL_ROOT"
mkdir -p "$INSTALL_ROOT"
mv "$STAGE/chrome-headless-shell-${PLATFORM}" "$INSTALL_ROOT/"
chmod 755 "$BROWSER_BIN"
printf '%s\n' "$BROWSER_BIN"

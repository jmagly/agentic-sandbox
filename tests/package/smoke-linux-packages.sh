#!/usr/bin/env bash
# Smoke-test Linux release packages in clean package-manager containers.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PACKAGES_DIR="${PACKAGES_DIR:-dist/packages}"
DEB_IMAGE="${PACKAGE_SMOKE_DEB_IMAGE:-ubuntu:24.04@sha256:786a8b558f7be160c6c8c4a54f9a57274f3b4fb1491cf65146521ae77ff1dc54}"
RPM_IMAGE="${PACKAGE_SMOKE_RPM_IMAGE:-rockylinux:9@sha256:d7be1c094cc5845ee815d4632fe377514ee6ebcf8efaed6892889657e5ddaaa6}"
REQUIRED=false

usage() {
  cat <<'EOF'
Usage: tests/package/smoke-linux-packages.sh [--required]

Runs clean install/uninstall smoke tests for:
  - dist/packages/*.deb in a Debian/Ubuntu container
  - dist/packages/*.rpm in an RPM-family container

Environment:
  PACKAGES_DIR              Package directory (default: dist/packages)
  PACKAGE_SMOKE_DEB_IMAGE   Debian-family image (default: ubuntu:24.04 pinned by digest)
  PACKAGE_SMOKE_RPM_IMAGE   RPM-family image (default: rockylinux:9 pinned by digest)

Without --required, the script skips when Docker is unavailable. In CI release
jobs, pass --required so missing Docker fails the release gate.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --required) REQUIRED=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if ! command -v docker >/dev/null 2>&1; then
  if [ "$REQUIRED" = "true" ]; then
    echo "docker is required for package smoke tests" >&2
    exit 1
  fi
  echo "docker not found; skipping package smoke tests"
  exit 0
fi

DEB="$(find "$PACKAGES_DIR" -maxdepth 1 -name '*.deb' | head -1)"
RPM="$(find "$PACKAGES_DIR" -maxdepth 1 -name '*.rpm' | head -1)"
[ -n "$DEB" ] || { echo "missing .deb in $PACKAGES_DIR" >&2; exit 1; }
[ -n "$RPM" ] || { echo "missing .rpm in $PACKAGES_DIR" >&2; exit 1; }

PKG_ABS="$(cd "$PACKAGES_DIR" && pwd)"
DEB_NAME="$(basename "$DEB")"
RPM_NAME="$(basename "$RPM")"

docker run --rm --platform linux/amd64 \
  -v "$PKG_ABS:/packages:ro" \
  "$DEB_IMAGE" \
  bash -s -- "$DEB_NAME" <<'DEBTEST'
set -euo pipefail
DEB_NAME="$1"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y "/packages/${DEB_NAME}"
test -x /usr/bin/agentic-mgmt
test -x /usr/bin/agentic-host-runtime-daemon
test -x /usr/bin/vm-event-bridge
test -x /usr/bin/agent-client
test -x /usr/bin/sandboxctl
test -L /usr/bin/agentic-sandbox
test -f /etc/agentic-sandbox/management.env
test -f /etc/agentic-sandbox/agent.env
test -f /etc/agentic-sandbox/host-runtime.env
test -f /lib/systemd/system/agentic-mgmt.service
test -f /lib/systemd/system/agent-client.service
sandboxctl --version
agentic-sandbox --version
apt-get remove -y agentic-sandbox
test ! -e /usr/bin/sandboxctl
test ! -e /usr/bin/agentic-sandbox
DEBTEST

docker run --rm --platform linux/amd64 \
  -v "$PKG_ABS:/packages:ro" \
  "$RPM_IMAGE" \
  bash -s -- "$RPM_NAME" <<'RPMTEST'
set -euo pipefail
RPM_NAME="$1"
dnf install -y "/packages/${RPM_NAME}"
test -x /usr/bin/agentic-mgmt
test -x /usr/bin/agentic-host-runtime-daemon
test -x /usr/bin/vm-event-bridge
test -x /usr/bin/agent-client
test -x /usr/bin/sandboxctl
test -L /usr/bin/agentic-sandbox
test -f /etc/agentic-sandbox/management.env
test -f /etc/agentic-sandbox/agent.env
test -f /etc/agentic-sandbox/host-runtime.env
test -f /lib/systemd/system/agentic-mgmt.service
test -f /lib/systemd/system/agent-client.service
sandboxctl --version
agentic-sandbox --version
dnf remove -y agentic-sandbox
test ! -e /usr/bin/sandboxctl
test ! -e /usr/bin/agentic-sandbox
RPMTEST

echo "package smoke tests passed"

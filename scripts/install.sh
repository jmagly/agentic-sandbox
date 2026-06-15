#!/usr/bin/env bash
# Agentic Sandbox one-line Linux installer.
#
# Downloads a release .deb or .rpm, verifies it against
# SHA256SUMS-linux-packages, installs it with the host package manager, and
# smoke-checks the installed CLI.

set -euo pipefail

AGENTIC_VERSION="${AGENTIC_VERSION:-latest}"
PACKAGE_FORMAT="${AGENTIC_PACKAGE_FORMAT:-auto}"
LOCAL_PACKAGE=""
DRY_RUN=false
SKIP_SMOKE_TEST=false
RELEASE_BASE="${AGENTIC_RELEASE_BASE:-https://github.com/jmagly/agentic-sandbox}"
RELEASE_API="${AGENTIC_RELEASE_API:-https://api.github.com/repos/jmagly/agentic-sandbox}"
TMPDIR_AGENTIC="$(mktemp -d -t agentic-sandbox-install.XXXXXX)"
trap 'rm -rf "$TMPDIR_AGENTIC"' EXIT

usage() {
  cat <<'EOF'
Usage: scripts/install.sh [options]

Options:
  --version <vX.Y.Z>       Install a specific release tag (default: latest)
  --package <auto|deb|rpm> Select package format (default: auto)
  --local-package <path>   Install a local .deb or .rpm instead of downloading
  --local-deb <path>       Alias for --local-package <path>
  --local-rpm <path>       Alias for --local-package <path>
  --dry-run                Resolve and validate inputs without installing
  --skip-smoke-test        Skip installed binary smoke checks
  -h, --help               Show this help

Environment:
  AGENTIC_RELEASE_BASE     Release download base URL
  AGENTIC_RELEASE_API      GitHub-compatible releases API URL
  AGENTIC_PACKAGE_FORMAT   auto, deb, or rpm
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) AGENTIC_VERSION="${2:-}"; shift 2 ;;
    --package) PACKAGE_FORMAT="${2:-}"; shift 2 ;;
    --local-package|--local-deb|--local-rpm) LOCAL_PACKAGE="${2:-}"; shift 2 ;;
    --dry-run) DRY_RUN=true; shift ;;
    --skip-smoke-test) SKIP_SMOKE_TEST=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

info() { printf '\033[0;32m[agentic-sandbox]\033[0m %s\n' "$*"; }
warn() { printf '\033[0;33m[agentic-sandbox]\033[0m %s\n' "$*"; }
die()  { printf '\033[0;31m[agentic-sandbox]\033[0m %s\n' "$*" >&2; exit 1; }

case "$PACKAGE_FORMAT" in
  auto|deb|rpm) ;;
  *) die "--package must be auto, deb, or rpm" ;;
esac

if [ "$(uname -s)" != "Linux" ]; then
  die "this installer supports Linux only"
fi

case "$(uname -m)" in
  x86_64|amd64) ;;
  aarch64|arm64) die "no Linux arm64 native package is published yet; use Docker or host-direct source builds" ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

if [ "$DRY_RUN" != "true" ]; then
  if [ "${EUID}" -eq 0 ]; then
    SUDO=""
  else
    command -v sudo >/dev/null || die "sudo required, or run as root"
    SUDO="sudo"
  fi
else
  SUDO=""
fi

command -v curl >/dev/null || die "curl is required"
command -v sha256sum >/dev/null || die "sha256sum is required"

detect_package_format() {
  if [ "$PACKAGE_FORMAT" != "auto" ]; then
    printf '%s\n' "$PACKAGE_FORMAT"
    return 0
  fi
  if command -v dpkg >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1; then
    printf 'deb\n'
    return 0
  fi
  if command -v rpm >/dev/null 2>&1; then
    printf 'rpm\n'
    return 0
  fi
  die "could not detect deb or rpm package manager"
}

resolve_latest() {
  info "resolving latest release" >&2
  local tag=""
  if command -v python3 >/dev/null 2>&1; then
    tag="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${RELEASE_API}/releases?per_page=20" \
      | python3 -c 'import json, re, sys
data=json.load(sys.stdin)
for rel in data:
    tag=rel.get("tag_name","")
    if not rel.get("draft") and not rel.get("prerelease") and re.match(r"^v[0-9]+\.[0-9]+\.[0-9]+$", tag):
        print(tag)
        break')"
  else
    tag="$(curl -fsSL -H 'Accept: application/vnd.github+json' "${RELEASE_API}/releases?per_page=20" \
      | grep -oE '"tag_name"[[:space:]]*:[[:space:]]*"v[0-9]+\.[0-9]+\.[0-9]+"' \
      | sed -E 's/.*"(v[0-9]+\.[0-9]+\.[0-9]+)"/\1/' \
      | head -1)"
  fi
  [ -n "$tag" ] || die "could not resolve latest release; pass --version vX.Y.Z"
  printf '%s\n' "$tag"
}

FORMAT="$(detect_package_format)"
PACKAGE_VERSION=""

if [ -n "$LOCAL_PACKAGE" ]; then
  [ -f "$LOCAL_PACKAGE" ] || die "local package not found: $LOCAL_PACKAGE"
  PACKAGE_PATH="$(readlink -f "$LOCAL_PACKAGE")"
  PACKAGE_NAME="$(basename "$PACKAGE_PATH")"
  case "$PACKAGE_NAME" in
    *.deb)
      FORMAT="deb"
      if command -v dpkg-deb >/dev/null 2>&1; then
        PACKAGE_VERSION="$(dpkg-deb -f "$PACKAGE_PATH" Version 2>/dev/null || true)"
      fi
      ;;
    *.rpm)
      FORMAT="rpm"
      if command -v rpm >/dev/null 2>&1; then
        PACKAGE_VERSION="$(rpm -qp --qf '%{VERSION}-%{RELEASE}' "$PACKAGE_PATH" 2>/dev/null || true)"
      fi
      ;;
    *) die "--local-package must point to a .deb or .rpm" ;;
  esac
  info "using local package: $PACKAGE_PATH"
else
  if [ "$AGENTIC_VERSION" = "latest" ]; then
    AGENTIC_VERSION="$(resolve_latest)"
  fi
  case "$AGENTIC_VERSION" in
    v[0-9]*.[0-9]*.[0-9]*) ;;
    *) die "--version must look like v2026.6.1" ;;
  esac
  VERSION_NO_V="${AGENTIC_VERSION#v}"
  case "$FORMAT" in
    deb) PACKAGE_NAME="agentic-sandbox_${VERSION_NO_V}-1_amd64.deb" ;;
    rpm) PACKAGE_NAME="agentic-sandbox-${VERSION_NO_V}-1.x86_64.rpm" ;;
  esac
  PACKAGE_VERSION="${VERSION_NO_V}-1"
  PACKAGE_PATH="${TMPDIR_AGENTIC}/${PACKAGE_NAME}"
  SUMS_PATH="${TMPDIR_AGENTIC}/SHA256SUMS-linux-packages"
  PACKAGE_URL="${RELEASE_BASE}/releases/download/${AGENTIC_VERSION}/${PACKAGE_NAME}"
  SUMS_URL="${RELEASE_BASE}/releases/download/${AGENTIC_VERSION}/SHA256SUMS-linux-packages"

  info "downloading ${PACKAGE_NAME}"
  curl -fL --progress-bar -o "$PACKAGE_PATH" "$PACKAGE_URL" \
    || die "failed to download $PACKAGE_URL"

  info "downloading SHA256SUMS-linux-packages"
  curl -fsSL -o "$SUMS_PATH" "$SUMS_URL" \
    || die "checksum manifest is required but was not available: $SUMS_URL"

  EXPECTED="$(awk -v target="$PACKAGE_NAME" '$2 == target { print $1; exit }' "$SUMS_PATH")"
  [ -n "$EXPECTED" ] || die "checksum for $PACKAGE_NAME not found in SHA256SUMS-linux-packages"
  ACTUAL="$(sha256sum "$PACKAGE_PATH" | awk '{print $1}')"
  [ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch for $PACKAGE_NAME"
  info "checksum OK"
fi

if [ "$DRY_RUN" = "true" ]; then
  info "dry run OK: format=${FORMAT} package=${PACKAGE_NAME}"
  exit 0
fi

installed_version() {
  case "$FORMAT" in
    deb)
      command -v dpkg-query >/dev/null 2>&1 || return 1
      dpkg-query -W -f='${Version}' agentic-sandbox 2>/dev/null
      ;;
    rpm)
      command -v rpm >/dev/null 2>&1 || return 1
      rpm -q --qf '%{VERSION}-%{RELEASE}' agentic-sandbox 2>/dev/null
      ;;
  esac
}

if [ -n "$PACKAGE_VERSION" ]; then
  CURRENT_VERSION="$(installed_version || true)"
  if [ "$CURRENT_VERSION" = "$PACKAGE_VERSION" ]; then
    info "agentic-sandbox ${PACKAGE_VERSION} is already installed; skipping package install"
    SKIP_PACKAGE_INSTALL=true
  else
    SKIP_PACKAGE_INSTALL=false
  fi
else
  SKIP_PACKAGE_INSTALL=false
fi

if [ "$SKIP_PACKAGE_INSTALL" != "true" ]; then
  case "$FORMAT" in
    deb)
      command -v apt-get >/dev/null || die "apt-get is required to install .deb packages"
      info "installing .deb with apt"
      ${SUDO} apt-get update -qq
      ${SUDO} apt-get install -y "$PACKAGE_PATH"
      ;;
    rpm)
      if command -v dnf >/dev/null 2>&1; then
        info "installing .rpm with dnf"
        ${SUDO} dnf install -y "$PACKAGE_PATH"
      elif command -v yum >/dev/null 2>&1; then
        info "installing .rpm with yum"
        ${SUDO} yum install -y "$PACKAGE_PATH"
      elif command -v zypper >/dev/null 2>&1; then
        info "installing .rpm with zypper"
        ${SUDO} zypper --non-interactive install "$PACKAGE_PATH"
      else
        die "dnf, yum, or zypper is required to install .rpm packages"
      fi
      ;;
  esac
fi

if [ "$SKIP_SMOKE_TEST" = "true" ]; then
  exit 0
fi

info "smoke-testing installed binaries"
command -v sandboxctl >/dev/null || die "sandboxctl was not installed on PATH"
command -v agentic-sandbox >/dev/null || die "agentic-sandbox CLI alias was not installed on PATH"
command -v agentic-mgmt >/dev/null || die "agentic-mgmt was not installed on PATH"
command -v agent-client >/dev/null || die "agent-client was not installed on PATH"
sandboxctl --version
agentic-sandbox --version
info "install complete"

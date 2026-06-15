#!/usr/bin/env bash
# Build x86_64 Linux .deb and .rpm release packages from already-built binaries.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=""
OUT_DIR="$ROOT/dist/packages"
TARGET="x86_64-unknown-linux-gnu"
REVISION="${AGENTIC_PACKAGE_REVISION:-1}"

usage() {
  cat <<'EOF'
Usage: scripts/package-linux.sh --version <version> [--out-dir <dir>]

Builds:
  dist/packages/agentic-sandbox_<version>-<revision>_amd64.deb
  dist/packages/agentic-sandbox-<version>-<revision>.x86_64.rpm

The script expects release binaries to already exist under each crate's
target/<target>/release directory. It does not run cargo builds.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[ -n "$VERSION" ] || { echo "missing --version" >&2; usage >&2; exit 2; }
VERSION="${VERSION#v}"

case "$VERSION" in
  *[!0-9.]*|"") echo "package version must be CalVer-like digits/dots: $VERSION" >&2; exit 2 ;;
esac

require_file() {
  local path="$1"
  [ -x "$path" ] || { echo "required executable not found: $path" >&2; exit 1; }
}

require_file "management/target/${TARGET}/release/agentic-mgmt"
require_file "management/target/${TARGET}/release/agentic-host-runtime-daemon"
require_file "management/target/${TARGET}/release/vm-event-bridge"
require_file "agent-rs/target/${TARGET}/release/agent-client"
require_file "cli/target/${TARGET}/release/sandboxctl"

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

PKG_NAME="agentic-sandbox"
ARCH_DEB="amd64"
ARCH_RPM="x86_64"
MAINTAINER="Agentic Sandbox Maintainers <maintainers@example.invalid>"
SUMMARY="Runtime isolation platform for persistent AI agent processes"
DESCRIPTION="Agentic Sandbox provides management, agent, CLI, and host-runtime binaries for QEMU/KVM, container, and host-direct agent execution."

stage_payload() {
  local stage="$1"
  mkdir -p \
    "$stage/usr/bin" \
    "$stage/etc/agentic-sandbox" \
    "$stage/lib/systemd/system" \
    "$stage/usr/share/doc/agentic-sandbox"

  install -m 0755 "management/target/${TARGET}/release/agentic-mgmt" "$stage/usr/bin/agentic-mgmt"
  install -m 0755 "management/target/${TARGET}/release/agentic-host-runtime-daemon" "$stage/usr/bin/agentic-host-runtime-daemon"
  install -m 0755 "management/target/${TARGET}/release/vm-event-bridge" "$stage/usr/bin/vm-event-bridge"
  install -m 0755 "agent-rs/target/${TARGET}/release/agent-client" "$stage/usr/bin/agent-client"
  install -m 0755 "cli/target/${TARGET}/release/sandboxctl" "$stage/usr/bin/sandboxctl"
  ln -s sandboxctl "$stage/usr/bin/agentic-sandbox"

  install -m 0644 deploy/packaging/env/management.env "$stage/etc/agentic-sandbox/management.env"
  install -m 0644 deploy/packaging/env/agent.env "$stage/etc/agentic-sandbox/agent.env"
  install -m 0644 deploy/packaging/env/host-runtime.env "$stage/etc/agentic-sandbox/host-runtime.env"

  install -m 0644 deploy/packaging/systemd/agentic-mgmt.service "$stage/lib/systemd/system/agentic-mgmt.service"
  install -m 0644 deploy/packaging/systemd/agentic-host-runtime-daemon.service "$stage/lib/systemd/system/agentic-host-runtime-daemon.service"
  install -m 0644 deploy/packaging/systemd/agent-client.service "$stage/lib/systemd/system/agent-client.service"

  install -m 0644 README.md "$stage/usr/share/doc/agentic-sandbox/README.md"
  install -m 0644 LICENSE "$stage/usr/share/doc/agentic-sandbox/LICENSE"
  install -m 0644 CHANGELOG.md "$stage/usr/share/doc/agentic-sandbox/CHANGELOG.md"
}

build_deb() {
  command -v dpkg-deb >/dev/null || { echo "dpkg-deb is required" >&2; exit 1; }
  local stage="$OUT_DIR/deb-root"
  stage_payload "$stage"
  mkdir -p "$stage/DEBIAN"
  cat > "$stage/DEBIAN/control" <<EOF
Package: ${PKG_NAME}
Version: ${VERSION}-${REVISION}
Section: admin
Priority: optional
Architecture: ${ARCH_DEB}
Maintainer: ${MAINTAINER}
Depends: libc6, libgcc-s1, libvirt0, libvirt-clients, ca-certificates
Conflicts: agentic-sandbox-cli, agentic-mgmt, agent-client
Replaces: agentic-sandbox-cli, agentic-mgmt, agent-client
Homepage: https://github.com/jmagly/agentic-sandbox
Description: ${SUMMARY}
 ${DESCRIPTION}
EOF
  dpkg-deb --build --root-owner-group "$stage" "$OUT_DIR/${PKG_NAME}_${VERSION}-${REVISION}_${ARCH_DEB}.deb"
}

build_rpm() {
  command -v rpmbuild >/dev/null || { echo "rpmbuild is required" >&2; exit 1; }
  local top="$OUT_DIR/rpmbuild"
  local buildroot="$top/BUILDROOT/${PKG_NAME}-${VERSION}-${REVISION}.${ARCH_RPM}"
  rm -rf "$top"
  mkdir -p "$top/BUILD" "$top/BUILDROOT" "$top/RPMS" "$top/SOURCES" "$top/SPECS" "$top/SRPMS" "$top/TMP" "$top/RPMDB"
  stage_payload "$buildroot"
  cat > "$top/SPECS/${PKG_NAME}.spec" <<EOF
Name: ${PKG_NAME}
Version: ${VERSION}
Release: ${REVISION}%{?dist}
Summary: ${SUMMARY}
License: AGPL-3.0-only
URL: https://github.com/jmagly/agentic-sandbox
Requires: libvirt-libs
Requires: libvirt-client
Requires: ca-certificates
Conflicts: agentic-sandbox-cli
Conflicts: agentic-mgmt
Conflicts: agent-client
BuildArch: ${ARCH_RPM}

%description
${DESCRIPTION}

%files
%license /usr/share/doc/agentic-sandbox/LICENSE
%doc /usr/share/doc/agentic-sandbox/README.md
%doc /usr/share/doc/agentic-sandbox/CHANGELOG.md
/usr/bin/agentic-mgmt
/usr/bin/agentic-host-runtime-daemon
/usr/bin/vm-event-bridge
/usr/bin/agent-client
/usr/bin/sandboxctl
/usr/bin/agentic-sandbox
%config(noreplace) /etc/agentic-sandbox/management.env
%config(noreplace) /etc/agentic-sandbox/agent.env
%config(noreplace) /etc/agentic-sandbox/host-runtime.env
/lib/systemd/system/agentic-mgmt.service
/lib/systemd/system/agentic-host-runtime-daemon.service
/lib/systemd/system/agent-client.service
EOF
  TMPDIR="$top/TMP" rpmbuild \
    --define "_topdir $top" \
    --define "_tmppath $top/TMP" \
    --define "_dbpath $top/RPMDB" \
    --buildroot "$buildroot" \
    -bb "$top/SPECS/${PKG_NAME}.spec"
  find "$top/RPMS" -type f -name '*.rpm' -exec cp -v {} "$OUT_DIR/" \;
}

build_deb
build_rpm

install -m 0644 scripts/install.sh "$OUT_DIR/agentic-sandbox-install.sh"

( cd "$OUT_DIR" && sha256sum *.deb *.rpm agentic-sandbox-install.sh > SHA256SUMS-linux-packages )
( cd "$OUT_DIR" && for f in *.deb *.rpm agentic-sandbox-install.sh; do sha256sum "$f" > "$f.sha256"; done )

rm -rf "$OUT_DIR/deb-root" "$OUT_DIR/rpmbuild"
ls -la "$OUT_DIR"
cat "$OUT_DIR/SHA256SUMS-linux-packages"

#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HELPER="${REPO_ROOT}/scripts/prepare-debian-apt-https.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

mkdir -p \
    "${TEST_ROOT}/etc/apt/apt.conf.d" \
    "${TEST_ROOT}/etc/apt/sources.list.d" \
    "${TEST_ROOT}/etc/ssl/certs"
printf '%s\n' 'synthetic public CA bundle' \
    > "${TEST_ROOT}/etc/ssl/certs/ca-certificates.crt"
cat > "${TEST_ROOT}/etc/apt/sources.list.d/debian.sources" <<'SOURCES'
# http://snapshot.debian.org/archive/debian/20250721T000000Z
Types: deb
URIs: http://deb.debian.org/debian
Suites: bookworm bookworm-updates
Components: main

Types: deb
URIs: http://deb.debian.org/debian-security
Suites: bookworm-security
Components: main
SOURCES
cat > "${TEST_ROOT}/etc/apt/sources.list" <<'SOURCES'
deb http://security.debian.org/debian-security bookworm-security main
SOURCES

sh "${HELPER}" "${TEST_ROOT}" >/dev/null
grep -q 'URIs: https://deb.debian.org/debian' \
    "${TEST_ROOT}/etc/apt/sources.list.d/debian.sources"
grep -q 'deb https://security.debian.org/debian-security' \
    "${TEST_ROOT}/etc/apt/sources.list"
grep -q 'Acquire::https::CaInfo "/etc/ssl/certs/ca-certificates.crt";' \
    "${TEST_ROOT}/etc/apt/apt.conf.d/99agentic-verified-https"
grep -q 'Acquire::https::Verify-Peer "true";' \
    "${TEST_ROOT}/etc/apt/apt.conf.d/99agentic-verified-https"
grep -q 'Acquire::https::Verify-Host "true";' \
    "${TEST_ROOT}/etc/apt/apt.conf.d/99agentic-verified-https"
if grep -R -Ev '^[[:space:]]*#' "${TEST_ROOT}/etc/apt" \
    | grep -Eq 'http://[^[:space:]]*debian\.org'
then
    echo "plain HTTP Debian source survived conversion" >&2
    exit 1
fi

rm -f "${TEST_ROOT}/etc/ssl/certs/ca-certificates.crt"
if sh "${HELPER}" "${TEST_ROOT}" >/dev/null 2>&1; then
    echo "helper accepted a missing CA bundle" >&2
    exit 1
fi

printf '%s\n' 'synthetic public CA bundle' \
    > "${TEST_ROOT}/etc/ssl/certs/ca-certificates.crt"
printf '%s\n' 'deb http://ftp.debian.org/debian bookworm main' \
    > "${TEST_ROOT}/etc/apt/sources.list"
if sh "${HELPER}" "${TEST_ROOT}" >/dev/null 2>&1; then
    echo "helper accepted an unconverted Debian mirror" >&2
    exit 1
fi

echo "Debian apt HTTPS bootstrap tests passed"

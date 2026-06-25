#!/usr/bin/env bash
# Regression coverage for build-base-image.sh automation safety (#585).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=../build-base-image.sh
# shellcheck disable=SC1091
source images/qemu/build-base-image.sh

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_success() {
    local name="$1"
    shift
    "$@" || fail "$name"
    echo "PASS: $name"
}

assert_failure() {
    local name="$1"
    shift
    if "$@"; then
        fail "$name unexpectedly succeeded"
    fi
    echo "PASS: $name"
}

tmpdir="$(mktemp -d)"
trap 'chmod -R u+w "$tmpdir" 2>/dev/null || true; rm -rf "$tmpdir"' EXIT

image="$tmpdir/existing.qcow2"
touch "$image"

assert_failure \
    "existing image refuses non-interactive overwrite without --force" \
    confirm_overwrite_existing_image "$image" false

BASE_DIR="$tmpdir"
default_image="$BASE_DIR/ubuntu-server-24.04-agent.qcow2"
touch "$default_image"
# shellcheck disable=SC2317
resolve_iso_path() { echo "$tmpdir/fake.iso"; }
# shellcheck disable=SC2317
verify_iso() { return 0; }
assert_failure \
    "build_image aborts non-interactive existing image before qemu-img" \
    build_image 24.04 1G 512 1 "" false false

assert_success \
    "existing image allows explicit --force overwrite" \
    confirm_overwrite_existing_image "$image" true

writable_dir="$tmpdir/writable"
assert_success \
    "output directory is created when parent is writable" \
    ensure_output_directory_writable "$writable_dir/out.qcow2" false

readonly_parent="$tmpdir/readonly"
mkdir "$readonly_parent"
chmod 0555 "$readonly_parent"
assert_failure \
    "unwritable BASE_DIR/output directory fails before image creation" \
    ensure_output_directory_writable "$readonly_parent/out.qcow2" false

# shellcheck disable=SC2317
resolve_iso_path() {
    fail "resolve_iso_path should not run when output directory is unwritable"
}
assert_failure \
    "build_image fails unwritable output directory before ISO verification" \
    build_image 24.04 1G 512 1 "$readonly_parent/out.qcow2" false false

echo "build-base-image automation tests passed"

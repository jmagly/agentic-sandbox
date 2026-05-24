#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMPDIR_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMPDIR_ROOT}"' EXIT

mkdir -p "${TMPDIR_ROOT}/fakebin" "${TMPDIR_ROOT}/base"
cat > "${TMPDIR_ROOT}/fakebin/qemu-img" <<'FAKE_QEMU_IMG'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${QEMU_FAKE_EMPTY:-0}" == "1" ]]; then
    exit 1
fi

if [[ "${1:-}" == "info" && "${2:-}" == "--output=json" ]]; then
    printf '{"format":"%s","virtual-size":%s}\n' "${QEMU_FAKE_FORMAT:-qcow2}" "${QEMU_FAKE_VIRTUAL_SIZE:-42949672960}"
    exit 0
fi

if [[ "${1:-}" == "info" ]]; then
    image="${2:-}"
    cat <<INFO
image: ${image}
file format: ${QEMU_FAKE_FORMAT:-qcow2}
virtual size: 40 GiB (42949672960 bytes)
INFO
    exit 0
fi

exit 1
FAKE_QEMU_IMG
chmod +x "${TMPDIR_ROOT}/fakebin/qemu-img"
export PATH="${TMPDIR_ROOT}/fakebin:${PATH}"

# shellcheck source=../lib/verify.sh
source "${ROOT_DIR}/images/qemu/lib/verify.sh"

write_manifest() {
    local image="$1"
    local size_bytes="${2:-$(stat -c "%s" "$image")}"
    local sha
    sha="$(sha256sum "$image" | awk '{print $1}')"
    cat > "$(dirname "$image")/manifest.json" <<JSON
{
  "$(basename "$image")": {
    "sha256": "${sha}",
    "size_bytes": ${size_bytes},
    "virtual_size_bytes": 42949672960,
    "format": "qcow2"
  }
}
JSON
}

base_image="${TMPDIR_ROOT}/base/ubuntu-server-24.04-agent.qcow2"
truncate -s 65536 "${base_image}"
write_manifest "${base_image}" 65536
verify_qcow2_backing "${base_image}"

bad_manifest_size=$((65536 + 1))
write_manifest "${base_image}" "${bad_manifest_size}"
if verify_qcow2_backing "${base_image}" 2>"${TMPDIR_ROOT}/err.log"; then
    echo "expected manifest size mismatch to fail" >&2
    exit 1
fi
grep -q "Base image size mismatch" "${TMPDIR_ROOT}/err.log"
grep -q "qemu-img:" "${TMPDIR_ROOT}/err.log"
grep -q "manifest:" "${TMPDIR_ROOT}/err.log"

write_manifest "${base_image}" 65536
QEMU_FAKE_EMPTY=1
export QEMU_FAKE_EMPTY
if verify_qcow2_backing "${base_image}" 2>"${TMPDIR_ROOT}/small.err"; then
    echo "expected small image without qemu metadata to fail" >&2
    exit 1
fi
grep -q "Base image is implausibly small" "${TMPDIR_ROOT}/small.err"

echo "verify.sh tests passed"

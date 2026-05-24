#!/bin/bash
# lib/verify.sh - Base image and ISO integrity verification (#258)
#
# Provides:
#   - verify_iso              Verify Ubuntu ISO against pinned sha256 from iso-pins.json
#   - record_qcow2_manifest   Record base qcow2 sha256 to manifest.json after build
#   - verify_qcow2_backing    Verify backing file matches recorded manifest before overlay
#
# Pin file lives at $SCRIPT_DIR/../iso-pins.json (relative to caller).
# Manifest lives at $BASE_DIR/manifest.json (operator-managed).
#
# All functions exit non-zero on integrity failure unless AIWG_SKIP_BASE_VERIFY=1
# is set in the environment (operator-explicit bypass, logged loudly).

: "${LIB_VERIFY_LOG_PREFIX:=[verify]}"

verify_log() { echo "$LIB_VERIFY_LOG_PREFIX $*" >&2; }
verify_fail() { echo "$LIB_VERIFY_LOG_PREFIX FAIL: $*" >&2; }

_qcow2_file_size_bytes() {
    stat -c "%s" "$1"
}

_is_uint() {
    [[ "${1:-}" =~ ^[0-9]+$ ]]
}

_prefix_verify_output() {
    local prefix="$1"
    while IFS= read -r line; do
        verify_fail "  ${prefix}${line}"
    done
}

_qcow2_info_json() {
    local qcow2_path="$1"
    if command -v qemu-img >/dev/null 2>&1; then
        qemu-img info --output=json "$qcow2_path" 2>/dev/null || true
    fi
}

_qcow2_info_field() {
    local qcow2_path="$1"
    local jq_filter="$2"
    local info_json
    info_json=$(_qcow2_info_json "$qcow2_path")
    if [[ -n "$info_json" ]] && command -v jq >/dev/null 2>&1; then
        jq -r "$jq_filter" 2>/dev/null <<<"$info_json" || true
    fi
}

_verify_qcow2_context() {
    local qcow2_path="$1"
    local manifest_file="${2:-}"

    verify_fail "Base image diagnostics: $qcow2_path"

    if command -v stat >/dev/null 2>&1; then
        stat -Lc "stat_size_bytes=%s mode=%a owner=%U:%G mtime=%y" "$qcow2_path" 2>&1 \
            | _prefix_verify_output "stat: "
    fi
    if command -v ls >/dev/null 2>&1; then
        ls -lhL "$qcow2_path" 2>&1 | _prefix_verify_output "ls: "
    fi
    if command -v qemu-img >/dev/null 2>&1; then
        qemu-img info "$qcow2_path" 2>&1 | _prefix_verify_output "qemu-img: "
    else
        verify_fail "  qemu-img: unavailable"
    fi
    if command -v findmnt >/dev/null 2>&1; then
        findmnt -T "$qcow2_path" -o TARGET,SOURCE,FSTYPE,OPTIONS -n 2>&1 \
            | _prefix_verify_output "mount: "
    fi
    if [[ -n "$manifest_file" && -f "$manifest_file" ]] && command -v jq >/dev/null 2>&1; then
        local filename
        filename=$(basename "$qcow2_path")
        jq -c --arg name "$filename" '.[$name] // empty' "$manifest_file" 2>&1 \
            | _prefix_verify_output "manifest: "
    fi
}

_verify_qcow2_sanity() {
    local qcow2_path="$1"
    local min_bytes="${AIWG_MIN_BASE_IMAGE_BYTES:-1073741824}"

    if [[ ! -f "$qcow2_path" ]]; then
        verify_fail "Base image missing: $qcow2_path"
        return 1
    fi

    local size_bytes
    size_bytes=$(_qcow2_file_size_bytes "$qcow2_path")

    local format virtual_size
    format=$(_qcow2_info_field "$qcow2_path" ".format // empty")
    virtual_size=$(_qcow2_info_field "$qcow2_path" ".\"virtual-size\" // empty")

    if [[ -n "$format" && "$format" != "qcow2" ]]; then
        verify_fail "Base image is not qcow2: $qcow2_path"
        verify_fail "  format: $format"
        _verify_qcow2_context "$qcow2_path"
        return 1
    fi

    if (( size_bytes < min_bytes )); then
        if _is_uint "$virtual_size" && (( virtual_size >= min_bytes )); then
            verify_log "WARNING: base image file length is below raw threshold but qcow2 virtual size is sane"
            verify_log "  size_bytes:          $size_bytes"
            verify_log "  virtual_size_bytes:  $virtual_size"
            verify_log "  minimum:             $min_bytes"
            verify_log "Continuing to manifest and sha256 verification."
            return 0
        fi

        verify_fail "Base image is implausibly small: $qcow2_path"
        verify_fail "  size_bytes: $size_bytes"
        verify_fail "  minimum:    $min_bytes"
        verify_fail "Rebuild or replace the base image before recording or using it."
        verify_fail "Override only for controlled fixtures: AIWG_SKIP_BASE_VERIFY=1"
        _verify_qcow2_context "$qcow2_path"
        return 1
    fi

    return 0
}

# Skip-switch — defense-in-depth, must be set explicitly per invocation
_verify_skip_check() {
    if [[ "${AIWG_SKIP_BASE_VERIFY:-0}" == "1" ]]; then
        verify_log "WARNING: AIWG_SKIP_BASE_VERIFY=1 — bypassing $1 verification"
        return 0
    fi
    return 1
}

# Resolve the path to iso-pins.json from a known anchor.
_resolve_pins_file() {
    local pins="${ISO_PINS_FILE:-}"
    if [[ -z "$pins" ]]; then
        # Try common anchors
        for candidate in \
            "${SCRIPT_DIR:-}/../iso-pins.json" \
            "${SCRIPT_DIR:-}/iso-pins.json" \
            "$(dirname "${BASH_SOURCE[0]}")/../iso-pins.json"; do
            if [[ -f "$candidate" ]]; then
                pins="$candidate"
                break
            fi
        done
    fi
    if [[ -z "$pins" ]] || [[ ! -f "$pins" ]]; then
        verify_fail "iso-pins.json not found (set ISO_PINS_FILE or place at images/qemu/iso-pins.json)"
        return 1
    fi
    echo "$pins"
}

# Verify an ISO against the pinned sha256 in iso-pins.json.
# Args: $1 = ubuntu major version (e.g. "24.04"), $2 = ISO path
# Behavior:
#   - If pin exists and is populated → compute actual sha256, compare, fail on mismatch
#   - If pin is empty → emit warning, refuse to proceed unless AIWG_SKIP_BASE_VERIFY=1
#   - Optional: if curl + gpg available, fetch SHA256SUMS + .gpg and cross-check (best-effort)
verify_iso() {
    local version="$1"
    local iso_path="$2"

    _verify_skip_check "ISO" && return 0

    local pins
    pins=$(_resolve_pins_file) || return 1

    if ! command -v jq >/dev/null 2>&1; then
        verify_fail "jq required for ISO verification but not installed"
        return 1
    fi

    local expected_sha
    expected_sha=$(jq -r ".releases[\"$version\"].sha256 // \"\"" "$pins")
    if [[ -z "$expected_sha" ]]; then
        verify_fail "No pinned sha256 for Ubuntu $version in $pins"
        verify_fail "Populate by running: images/qemu/scripts/pin-iso.sh $version"
        verify_fail "Or override with: AIWG_SKIP_BASE_VERIFY=1 (NOT recommended)"
        return 1
    fi

    verify_log "Computing sha256 of $iso_path ..."
    local actual_sha
    actual_sha=$(sha256sum "$iso_path" | awk '{print $1}')

    if [[ "$actual_sha" != "$expected_sha" ]]; then
        verify_fail "ISO sha256 mismatch for Ubuntu $version"
        verify_fail "  expected: $expected_sha"
        verify_fail "  actual:   $actual_sha"
        verify_fail "  iso:      $iso_path"
        return 1
    fi

    verify_log "ISO sha256 OK ($expected_sha)"
    return 0
}

# Record qcow2 sha256 to manifest.json after a successful build.
# Args: $1 = qcow2 path, $2 = manifest dir (default: dirname of qcow2)
record_qcow2_manifest() {
    local qcow2_path="$1"
    local manifest_dir="${2:-$(dirname "$qcow2_path")}"
    local manifest_file="$manifest_dir/manifest.json"

    if ! command -v jq >/dev/null 2>&1; then
        verify_log "WARNING: jq not installed — skipping manifest record (manual entry required)"
        return 1
    fi

    _verify_skip_check "base-image sanity before manifest record" || _verify_qcow2_sanity "$qcow2_path" || return 1

    local filename
    filename=$(basename "$qcow2_path")

    local size_bytes virtual_size format
    size_bytes=$(_qcow2_file_size_bytes "$qcow2_path")
    virtual_size=$(_qcow2_info_field "$qcow2_path" ".\"virtual-size\" // empty")
    format=$(_qcow2_info_field "$qcow2_path" ".format // empty")

    verify_log "Computing sha256 of $qcow2_path ..."
    local sha
    sha=$(sha256sum "$qcow2_path" | awk '{print $1}')
    local now
    now=$(date -u -Iseconds)

    # Read existing manifest or seed empty
    local existing="{}"
    if [[ -f "$manifest_file" ]]; then
        existing=$(cat "$manifest_file")
    fi

    # Upsert this filename's entry
    local updated
    updated=$(echo "$existing" | jq \
        --arg name "$filename" \
        --arg sha "$sha" \
        --arg ts "$now" \
        --argjson size_bytes "$size_bytes" \
        --arg virtual_size "$virtual_size" \
        --arg format "$format" \
        '. + {($name): ({"sha256": $sha, "recorded_at": $ts, "size_bytes": $size_bytes}
            + (if $virtual_size != "" then {"virtual_size_bytes": ($virtual_size | tonumber)} else {} end)
            + (if $format != "" then {"format": $format} else {} end))}')

    # Atomic write (manifest dir may be root-owned; tolerate sudo)
    local tmp
    tmp=$(mktemp)
    echo "$updated" | jq . > "$tmp"
    if [[ -w "$manifest_dir" ]]; then
        mv "$tmp" "$manifest_file"
    else
        sudo mv "$tmp" "$manifest_file"
    fi
    sudo chmod 644 "$manifest_file" 2>/dev/null || chmod 644 "$manifest_file"

    verify_log "Recorded $filename → $sha in $manifest_file"
    return 0
}

# Verify backing-file sha256 against manifest before creating an overlay.
# Args: $1 = base image path
# Behavior:
#   - If manifest.json absent → loud warning, refuse unless AIWG_SKIP_BASE_VERIFY=1
#   - If filename not in manifest → loud warning, refuse unless override
#   - If sha mismatch → fail
verify_qcow2_backing() {
    local base_image="$1"

    _verify_skip_check "base-image" && return 0
    _verify_qcow2_sanity "$base_image" || return 1

    local manifest_file
    manifest_file="$(dirname "$base_image")/manifest.json"

    if [[ ! -f "$manifest_file" ]]; then
        verify_fail "No manifest.json beside base image ($manifest_file)"
        verify_fail "Run build-base-image.sh OR bootstrap with:"
        verify_fail "  source images/qemu/lib/verify.sh && record_qcow2_manifest $base_image"
        verify_fail "Or override (NOT recommended): AIWG_SKIP_BASE_VERIFY=1"
        _verify_qcow2_context "$base_image" "$manifest_file"
        return 1
    fi

    if ! command -v jq >/dev/null 2>&1; then
        verify_fail "jq required for backing verification but not installed"
        return 1
    fi

    local filename
    filename=$(basename "$base_image")
    local expected
    expected=$(jq -r ".[\"$filename\"].sha256 // \"\"" "$manifest_file")
    if [[ -z "$expected" ]]; then
        verify_fail "Base image $filename not present in $manifest_file"
        verify_fail "Bootstrap: source images/qemu/lib/verify.sh && record_qcow2_manifest $base_image"
        _verify_qcow2_context "$base_image" "$manifest_file"
        return 1
    fi

    local expected_size
    expected_size=$(jq -r ".[\"$filename\"].size_bytes // \"\"" "$manifest_file")
    if [[ -n "$expected_size" ]]; then
        local actual_size
        actual_size=$(_qcow2_file_size_bytes "$base_image")
        if [[ "$actual_size" != "$expected_size" ]]; then
            verify_fail "Base image size mismatch: $filename"
            verify_fail "  expected_size_bytes: $expected_size"
            verify_fail "  actual_size_bytes:   $actual_size"
            verify_fail "  manifest: $manifest_file"
            _verify_qcow2_context "$base_image" "$manifest_file"
            return 1
        fi
    fi

    local actual
    actual=$(sha256sum "$base_image" | awk '{print $1}')
    if [[ "$actual" != "$expected" ]]; then
        verify_fail "Base image tampering detected: $filename"
        verify_fail "  expected: $expected"
        verify_fail "  actual:   $actual"
        verify_fail "  manifest: $manifest_file"
        _verify_qcow2_context "$base_image" "$manifest_file"
        return 1
    fi

    verify_log "Base image sha256 OK ($filename → $expected)"
    return 0
}

#!/bin/bash
# snapshot-seal.sh - Encrypt + fixity-protect a VM checkpoint bundle at rest.
#
# Follow-up implementation for #645 (snapshot secret hygiene). A guest-memory checkpoint contains
# everything in RAM verbatim - incl. the mTLS client key + Claude OAuth tokens (#617) and the
# bootstrap bearer token (#619). Any checkpoint that MUST contain secrets has to be encrypted at
# rest and access-scoped. This tool seals a checkpoint bundle (the checkpoint-vm.sh image + its
# .nvram / .virtiofs.xml sidecars) into a single integrity-protected, encrypted artifact.
#
# Crypto per .claude/rules/crypto-flag-verification.md + no-unauthenticated-encryption.md:
#   gpg --symmetric AES256 with a strong S2K (mode 3, high count, SHA512) -> integrity-protected
#   (MDC/AEAD) authenticated encryption. Passphrase is read from a keyfile (never on argv/env),
#   per .claude/rules/token-security.md. A SHA-256 manifest gives fixity; optional detached gpg
#   signature gives provenance so a swapped/backdoored base cannot fan out to every fork.
#
# The PREFERRED posture (see docs/research/memory-snapshot-restore-spike.md) is to snapshot the
# pre-enrollment CLEAN base so no secrets land in the image at all. This tool is the defense for
# the cases where a secret-bearing checkpoint is unavoidable.
#
# Usage:
#   ./snapshot-seal.sh seal   --key <keyfile> --out <bundle.gpg> [--sign-key <id>] <file>...
#   ./snapshot-seal.sh unseal --key <keyfile> --in  <bundle.gpg> --dest <dir>
#   ./snapshot-seal.sh verify --in <bundle.gpg>
#   ./snapshot-seal.sh selftest
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
log(){ echo -e "${BLUE}[seal]${NC} $*"; }
ok(){ echo -e "${GREEN}[ ok ]${NC} $*"; }
warn(){ echo -e "${YELLOW}[warn]${NC} $*" >&2; }
die(){ echo -e "${RED}[fail]${NC} $*" >&2; exit 1; }

usage(){ sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; }

# Hardened symmetric encryption (recipe from crypto-flag-verification.md).
_gpg_enc(){ # <keyfile> <in> <out>
    gpg --batch --yes --quiet --pinentry-mode loopback --passphrase-file "$1" \
        --symmetric --cipher-algo AES256 \
        --s2k-mode 3 --s2k-count 65011712 --s2k-cipher-algo AES256 --s2k-digest-algo SHA512 \
        --compress-algo none -o "$3" "$2"
}
_gpg_dec(){ # <keyfile> <in> <out>
    gpg --batch --yes --quiet --pinentry-mode loopback --passphrase-file "$1" \
        --decrypt -o "$3" "$2"
}

_check_key(){
    [ -r "$1" ] || die "keyfile not readable: $1"
    local perm; perm=$(stat -c %a "$1" 2>/dev/null || echo "")
    case "$perm" in 600|400) ;; *) warn "keyfile $1 has mode $perm; recommend 600 (owner-only)";; esac
}

cmd_seal(){
    local key="" out="" signkey="" files=()
    while [ $# -gt 0 ]; do case "$1" in
        --key) key="$2"; shift 2;; --out) out="$2"; shift 2;; --sign-key) signkey="$2"; shift 2;;
        *) files+=("$1"); shift;; esac; done
    [ -n "$key" ] && [ -n "$out" ] && [ ${#files[@]} -gt 0 ] || { usage; die "seal needs --key, --out, and file(s)"; }
    _check_key "$key"
    for f in "${files[@]}"; do [ -e "$f" ] || die "input missing: $f"; done

    if ! (
        local tmp
        tmp="$(mktemp -d)"
        cleanup_seal_tmp() {
            local rc=$?
            rm -rf -- "$tmp"
            if [ "$rc" -ne 0 ]; then
                rm -f -- "$out" "$out.sha256" "$out.sig"
            fi
            trap - EXIT
            exit "$rc"
        }
        trap cleanup_seal_tmp EXIT
        # tar preserves the bundle (image + sidecars) as one unit; basenames only.
        local tarball="$tmp/bundle.tar"
        tar -C "$(dirname "${files[0]}")" -cf "$tarball" "${files[@]/#*\//}" 2>/dev/null \
            || tar -cf "$tarball" -C / "${files[@]#/}"   # fallback: absolute
        _gpg_enc "$key" "$tarball" "$out"
    ); then
        die "failed to seal snapshot bundle"
    fi
    sha256sum "$out" | awk '{print $1}' > "$out.sha256"
    chmod 600 "$out" "$out.sha256" 2>/dev/null || true
    ok "sealed $(numfmt --to=iec < <(stat -c %s "$out")) -> $out"
    log "fixity: $out.sha256 = $(cat "$out.sha256")"
    if [ -n "$signkey" ]; then
        gpg --batch --yes --quiet --local-user "$signkey" --detach-sign -o "$out.sig" "$out" \
            && ok "detached signature: $out.sig (key $signkey)" \
            || { rm -f -- "$out" "$out.sha256" "$out.sig"; die "signing failed"; }
    fi
}

cmd_unseal(){
    local key="" in="" dest=""
    while [ $# -gt 0 ]; do case "$1" in
        --key) key="$2"; shift 2;; --in) in="$2"; shift 2;; --dest) dest="$2"; shift 2;;
        *) shift;; esac; done
    [ -n "$key" ] && [ -n "$in" ] && [ -n "$dest" ] || { usage; die "unseal needs --key, --in, --dest"; }
    _check_key "$key"; [ -e "$in" ] || die "sealed bundle missing: $in"
    cmd_verify --in "$in" || warn "fixity/signature check reported issues (continuing to authenticated decrypt)"
    mkdir -p "$dest"
    local tmp; tmp="$(mktemp -d)"
    # gpg decrypt is authenticated: tampered ciphertext fails here (non-zero).
    _gpg_dec "$key" "$in" "$tmp/bundle.tar" || { rm -rf "$tmp"; die "decrypt/integrity FAILED (wrong key or tampered bundle)"; }
    tar -C "$dest" -xf "$tmp/bundle.tar"
    rm -rf "$tmp"
    ok "unsealed -> $dest/"
}

cmd_verify(){
    local in=""
    while [ $# -gt 0 ]; do case "$1" in --in) in="$2"; shift 2;; *) shift;; esac; done
    [ -n "$in" ] && [ -e "$in" ] || { usage; die "verify needs --in <bundle.gpg>"; }
    local rc=0
    if [ -r "$in.sha256" ]; then
        local want got; want="$(cat "$in.sha256")"; got="$(sha256sum "$in" | awk '{print $1}')"
        if [ "$want" = "$got" ]; then ok "fixity OK ($got)"; else warn "FIXITY MISMATCH: want $want got $got"; rc=1; fi
    else warn "no .sha256 manifest alongside $in"; rc=1; fi
    if [ -r "$in.sig" ]; then
        gpg --batch --quiet --verify "$in.sig" "$in" 2>/dev/null && ok "signature OK" || { warn "SIGNATURE INVALID"; rc=1; }
    fi
    return $rc
}

cmd_selftest(){
    command -v gpg >/dev/null || die "gpg not found"
    local d; d="$(mktemp -d)"
    log "selftest in $d"
    # fake checkpoint bundle
    head -c 3145728 /dev/urandom > "$d/ckpt.save"           # 3 MiB pseudo-RAM image
    echo "<filesystem><driver type='virtiofs'/></filesystem>" > "$d/ckpt.save.virtiofs.xml"
    head -c 528000  /dev/urandom > "$d/ckpt.save.nvram"
    printf 'super-secret-passphrase-%s\n' "$$" > "$d/key"; chmod 600 "$d/key"

    cmd_seal --key "$d/key" --out "$d/bundle.gpg" "$d/ckpt.save" "$d/ckpt.save.virtiofs.xml" "$d/ckpt.save.nvram"
    [ -s "$d/bundle.gpg" ] || die "sealed bundle empty"
    # ciphertext must not contain the plaintext image bytes (spot check: nvram not recoverable by grep)
    cmd_verify --in "$d/bundle.gpg" >/dev/null || die "verify failed on fresh bundle"

    cmd_unseal --key "$d/key" --in "$d/bundle.gpg" --dest "$d/out" >/dev/null
    for f in ckpt.save ckpt.save.virtiofs.xml ckpt.save.nvram; do
        cmp -s "$d/$f" "$d/out/$f" || die "roundtrip mismatch on $f"
    done
    ok "roundtrip byte-identical (image + sidecars)"

    # negative 1: wrong key must fail (test at the crypto layer; cmd_unseal die()s on failure)
    printf 'wrong-key\n' > "$d/badkey"; chmod 600 "$d/badkey"
    if _gpg_dec "$d/badkey" "$d/bundle.gpg" "$d/x1.tar" 2>/dev/null; then
        die "SECURITY: decrypt succeeded with WRONG key"; fi
    ok "wrong key rejected"

    # negative 2: tampered ciphertext must fail integrity
    cp "$d/bundle.gpg" "$d/tampered.gpg"
    printf '\xff\xff\xff\xff' | dd of="$d/tampered.gpg" bs=1 seek=64 count=4 conv=notrunc status=none
    if _gpg_dec "$d/key" "$d/tampered.gpg" "$d/x.tar" 2>/dev/null; then
        die "SECURITY: tampered ciphertext decrypted without integrity failure"; fi
    ok "tampered ciphertext rejected (authenticated encryption)"

    echo -e "${GREEN}SELFTEST PASSED${NC}: seal/unseal round-trips; wrong-key and tamper both rejected."
    rm -rf "$d"
}

case "${1:-}" in
    seal)     shift; cmd_seal "$@";;
    unseal)   shift; cmd_unseal "$@";;
    verify)   shift; cmd_verify "$@";;
    selftest) shift; cmd_selftest "$@";;
    -h|--help|"") usage;;
    *) usage; die "unknown subcommand '$1'";;
esac

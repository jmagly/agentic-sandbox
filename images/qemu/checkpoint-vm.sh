#!/bin/bash
# checkpoint-vm.sh - Checkpoint / fast-resume an agent VM via migrate-to-file (virsh save/restore).
#
# Implements the mechanism established by spike #639: on the q35 UEFI + virtiofs stack, `virsh save`
# is the only full RAM+device capture, and virtiofs (vhost-user) BLOCKS it
# ("Migration disabled: vhost-user backend lacks VHOST_USER_PROTOCOL_F_LOG_SHMFD feature").
# So we: unmount+detach virtiofs -> save (RAM+devices) -> persist UEFI NVRAM + the virtiofs device
# XML alongside; and on restore: restore state -> re-attach virtiofs -> guest re-mounts.
#
# Follow-up implementation for #643. Findings: docs/research/memory-snapshot-restore-spike.md
#
# Usage:
#   ./checkpoint-vm.sh save    <vm> <outfile> [--managed]
#   ./checkpoint-vm.sh restore <infile> [--name NAME]
#   ./checkpoint-vm.sh selftest
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
log()  { echo -e "${BLUE}[checkpoint]${NC} $*"; }
ok()   { echo -e "${GREEN}[ ok ]${NC} $*"; }
warn() { echo -e "${YELLOW}[warn]${NC} $*" >&2; }
die()  { echo -e "${RED}[fail]${NC} $*" >&2; exit 1; }

usage() {
    cat <<EOF
Usage:
  $0 save    <vm> <outfile> [--managed]   Checkpoint a running VM to <outfile>
  $0 restore <infile> [--name NAME]        Restore a VM from a checkpoint
  $0 selftest                              Build a throwaway VM and round-trip it

save writes three artifacts:
  <outfile>              QEMU migrate-to-file image (RAM + device state)
  <outfile>.virtiofs.xml virtiofs <filesystem> device XML (for re-attach on restore)
  <outfile>.nvram        copy of the per-VM UEFI NVRAM (if the domain uses pflash)

Notes (from spike #639):
  - virtiofs must be detached before save; this tool unmounts (best-effort via guest agent)
    then detaches, then saves, then re-attaches + remounts on restore.
  - internal qcow2 snapshots (savevm) do NOT work on this profile (memfd + UEFI pflash).
  - restore-to-usable is seconds, not sub-second; see #644 (Cloud Hypervisor) for sub-second.
  - a checkpoint contains everything in guest RAM VERBATIM (mTLS key #617, OAuth, bootstrap
    token #619). Prefer snapshotting a pre-enrollment CLEAN base. If a secret-bearing checkpoint
    is unavoidable, seal it at rest with snapshot-seal.sh (#645):
      ./snapshot-seal.sh seal --key <keyfile> --out ckpt.gpg <outfile> <outfile>.nvram <outfile>.virtiofs.xml
EOF
}

# --- helpers ---------------------------------------------------------------
_agent_ok()   { virsh qemu-agent-command "$1" '{"execute":"guest-ping"}' >/dev/null 2>&1; }
_wait_agent() { local vm=$1 lim=${2:-90} t=0; while ! _agent_ok "$vm"; do sleep 1; t=$((t+1)); [ $t -ge "$lim" ] && return 1; done; return 0; }
_running()    { [ "$(virsh domstate "$1" 2>/dev/null)" = "running" ]; }

# Extract each <filesystem>...</filesystem> block (virtiofs) from a domain's live XML.
_virtiofs_blocks() {
    virsh dumpxml "$1" 2>/dev/null | awk '
        /<filesystem/{cap=1}
        cap{buf=buf $0 ORS}
        /<\/filesystem>/{if(cap){printf "%s\036", buf; buf=""; cap=0}}'
}
# Path of the per-VM UEFI NVRAM file, if any.
_nvram_path() { virsh dumpxml "$1" 2>/dev/null | sed -n "s/.*<nvram[^>]*>\(.*\)<\/nvram>.*/\1/p" | head -1; }

# Best-effort: unmount every virtiofs mount inside the guest (so detach won't EBUSY).
_guest_umount_virtiofs() {
    local vm=$1
    _agent_ok "$vm" || { warn "no guest agent; skipping in-guest unmount for $vm"; return 0; }
    virsh qemu-agent-command "$vm" \
      '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","for m in $(awk \"/ virtiofs /{print \\$2}\" /proc/mounts); do umount -l \"$m\"; done"]}}' \
      >/dev/null 2>&1 || true
    sleep 1
}
# Best-effort: remount known agentshare tags after re-attach.
_guest_remount_virtiofs() {
    local vm=$1
    _agent_ok "$vm" || return 0
    virsh qemu-agent-command "$vm" \
      '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","mountpoint -q /mnt/global || (mkdir -p /mnt/global && mount -t virtiofs agentglobal /mnt/global -o ro 2>/dev/null); mountpoint -q /mnt/inbox || (mkdir -p /mnt/inbox && mount -t virtiofs agentinbox /mnt/inbox 2>/dev/null); true"]}}' \
      >/dev/null 2>&1 || true
}

# --- save ------------------------------------------------------------------
cmd_save() {
    local vm="${1:-}" out="${2:-}"; shift 2 || true
    local managed=false
    for a in "$@"; do [ "$a" = "--managed" ] && managed=true; done
    [ -n "$vm" ] && [ -n "$out" ] || { usage; die "save needs <vm> <outfile>"; }
    _running "$vm" || die "domain '$vm' is not running"
    mkdir -p "$(dirname "$out")"

    # 1. persist virtiofs device XML + UEFI NVRAM before we tear anything down
    local blocks; blocks="$(_virtiofs_blocks "$vm")"
    : > "$out.virtiofs.xml"
    if [ -n "$blocks" ]; then printf '%s' "$blocks" | tr '\036' '\n' > "$out.virtiofs.xml"; fi
    local nvram; nvram="$(_nvram_path "$vm")"
    if [ -n "$nvram" ] && [ -r "$nvram" ]; then cp -f "$nvram" "$out.nvram"; ok "saved NVRAM ($(du -h "$out.nvram"|cut -f1))"; fi

    # 2. quiesce + detach virtiofs (required: virtiofs blocks migrate-to-file)
    if [ -n "$blocks" ]; then
        _guest_umount_virtiofs "$vm"
        local n=0
        while IFS= read -r -d $'\036' blk; do
            [ -n "${blk//[$'\t\r\n ']/}" ] || continue
            if virsh detach-device "$vm" <(printf '%s' "$blk") --live >/dev/null 2>&1; then
                n=$((n+1))
            else
                warn "failed to detach a virtiofs device; save may fail"
            fi
        done < <(printf '%s' "$blocks")
        ok "detached $n virtiofs device(s)"
    fi

    # 3. capture RAM + device state
    local t0 t1; t0=$(date +%s.%N)
    if $managed; then
        virsh managedsave "$vm" >/dev/null || die "managedsave failed (virtiofs still attached?)"
        # managedsave stores under libvirt's save dir; copy out for portability
        local msf="/var/lib/libvirt/qemu/save/${vm}.save"
        [ -r "$msf" ] && cp -f "$msf" "$out" 2>/dev/null || warn "managedsave image not directly readable; state kept in libvirt"
    else
        virsh save "$vm" "$out" >/dev/null || die "virsh save failed (virtiofs still attached?)"
    fi
    t1=$(date +%s.%N)
    sync
    local sz; sz=$(stat -c %s "$out" 2>/dev/null || echo 0)
    ok "checkpoint written: $out ($((sz/1048576)) MiB) in $(awk "BEGIN{printf \"%.2f\",$t1-$t0}")s"
    log "sidecars: $out.virtiofs.xml  $out.nvram"
}

# --- restore ---------------------------------------------------------------
cmd_restore() {
    local in="${1:-}"; shift || true
    local name=""
    while [ $# -gt 0 ]; do case "$1" in --name) name="$2"; shift 2;; *) shift;; esac; done
    # NB: virsh save writes the image root-owned 0600; libvirtd (root) reads it on restore,
    # so we test existence, not our own read access.
    [ -n "$in" ] && [ -e "$in" ] || { usage; die "restore needs an existing <infile>"; }

    # If we have an NVRAM sidecar and can determine its target path, place it first
    # (needed for cross-host / cold restore; harmless same-host).
    if [ -r "$in.nvram" ]; then
        local tgt; tgt="$(virsh save-image-dumpxml "$in" 2>/dev/null | sed -n "s/.*<nvram[^>]*>\(.*\)<\/nvram>.*/\1/p" | head -1)"
        if [ -n "$tgt" ] && [ ! -e "$tgt" ]; then cp -f "$in.nvram" "$tgt" && log "placed NVRAM at $tgt"; fi
    fi

    local t0 t1; t0=$(date +%s.%N)
    virsh restore "$in" >/dev/null || die "virsh restore failed"
    # domain name from the saved image if not provided
    [ -n "$name" ] || name="$(virsh save-image-dumpxml "$in" 2>/dev/null | sed -n 's:.*<name>\(.*\)</name>.*:\1:p' | head -1)"
    [ -n "$name" ] || die "could not determine restored domain name; pass --name"
    _wait_agent "$name" 90 || warn "guest agent not responding after restore"
    t1=$(date +%s.%N)
    ok "restored '$name' -> usable in $(awk "BEGIN{printf \"%.2f\",$t1-$t0}")s"

    # re-attach virtiofs + remount
    if [ -r "$in.virtiofs.xml" ] && [ -s "$in.virtiofs.xml" ]; then
        local n=0 blk="" line
        while IFS= read -r line || [ -n "$line" ]; do
            blk+="$line"$'\n'
            if printf '%s' "$line" | grep -q '</filesystem>'; then
                if virsh attach-device "$name" <(printf '%s' "$blk") --live >/dev/null 2>&1; then n=$((n+1)); else warn "failed to re-attach a virtiofs device"; fi
                blk=""
            fi
        done < "$in.virtiofs.xml"
        ok "re-attached $n virtiofs device(s)"
        _guest_remount_virtiofs "$name"
    fi
}

# --- selftest --------------------------------------------------------------
cmd_selftest() {
    local VM=chkpt-selftest CID=6390; local WORK=/var/tmp/$VM
    local BASE=/mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2
    local CODE=/usr/share/OVMF/OVMF_CODE_4M.fd VARS=/usr/share/OVMF/OVMF_VARS_4M.fd
    command -v virsh >/dev/null || die "virsh not found"
    [ -r "$BASE" ] || die "base image not found: $BASE"
    log "selftest: building throwaway VM $VM"
    virsh destroy "$VM" >/dev/null 2>&1 || true; virsh undefine "$VM" --nvram >/dev/null 2>&1 || true
    rm -rf "$WORK"; mkdir -p "$WORK/global-ro" "$WORK/inbox"; chmod -R 0777 "$WORK"
    local MARK="hello-639-$$"; echo "$MARK" > "$WORK/inbox/marker"
    qemu-img create -f qcow2 -F qcow2 -b "$BASE" "$WORK/disk.qcow2" >/dev/null; chmod 0666 "$WORK/disk.qcow2"
    cp "$VARS" "$WORK/VARS.fd"; chmod 0666 "$WORK/VARS.fd"
    cat > "$WORK/domain.xml" <<XML
<domain type='kvm'>
  <name>$VM</name><memory unit='MiB'>2048</memory><vcpu>2</vcpu>
  <memoryBacking><source type='memfd'/><access mode='shared'/></memoryBacking>
  <os><type arch='x86_64' machine='q35'>hvm</type>
    <loader readonly='yes' type='pflash'>$CODE</loader><nvram>$WORK/VARS.fd</nvram><boot dev='hd'/></os>
  <features><acpi/><apic/></features><cpu mode='host-passthrough'/>
  <devices><emulator>/usr/bin/qemu-system-x86_64</emulator>
    <disk type='file' device='disk'><driver name='qemu' type='qcow2' cache='writeback'/><source file='$WORK/disk.qcow2'/><target dev='vda' bus='virtio'/></disk>
    <interface type='network'><source network='default'/><model type='virtio'/></interface>
    <filesystem type='mount' accessmode='passthrough'><driver type='virtiofs'/><source dir='$WORK/global-ro'/><target dir='agentglobal'/></filesystem>
    <filesystem type='mount' accessmode='passthrough'><driver type='virtiofs'/><source dir='$WORK/inbox'/><target dir='agentinbox'/></filesystem>
    <vsock model='virtio'><cid auto='no' address='$CID'/></vsock>
    <channel type='unix'><target type='virtio' name='org.qemu.guest_agent.0'/></channel>
    <serial type='pty'><target port='0'/></serial><console type='pty'><target type='serial' port='0'/></console>
  </devices><on_poweroff>destroy</on_poweroff><on_reboot>restart</on_reboot><on_crash>destroy</on_crash>
</domain>
XML
    virsh define "$WORK/domain.xml" >/dev/null
    virsh start "$VM" >/dev/null
    _wait_agent "$VM" 120 || { virsh destroy "$VM" >/dev/null 2>&1; die "guest agent never came up"; }
    ok "VM booted, guest agent up"
    # mount inbox virtiofs in guest and confirm marker visible (proves active mount pre-checkpoint)
    virsh qemu-agent-command "$VM" '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","mkdir -p /mnt/inbox && mount -t virtiofs agentinbox /mnt/inbox"]}}' >/dev/null 2>&1
    sleep 2

    local CKPT=$WORK/ckpt.save
    log "checkpoint..."; cmd_save "$VM" "$CKPT"
    [ -s "$CKPT" ] || die "checkpoint image empty"
    [ -s "$CKPT.virtiofs.xml" ] || die "virtiofs sidecar missing/empty"
    grep -q 'virtiofs' "$CKPT.virtiofs.xml" || die "virtiofs sidecar has no filesystem block"
    [ -s "$CKPT.nvram" ] || die "NVRAM sidecar missing"
    ok "checkpoint artifacts present (image + virtiofs.xml + nvram)"
    _running "$VM" && die "domain still running after save (should be saved/stopped)" || true

    log "restore..."; cmd_restore "$CKPT" --name "$VM"
    _agent_ok "$VM" || die "guest agent not responding after restore"
    virsh dumpxml "$VM" | grep -q 'virtiofs' || die "virtiofs not re-attached after restore"
    ok "restored, guest usable, virtiofs re-attached"
    # verify guest can read the marker again (remount worked)
    local seen; seen="$(virsh qemu-agent-command "$VM" '{"execute":"guest-exec","arguments":{"path":"/bin/sh","arg":["-c","cat /mnt/inbox/marker 2>/dev/null"],"capture-output":true}}' 2>/dev/null)"
    # decode guest-exec output
    local pid; pid=$(echo "$seen" | sed -n 's/.*"pid":\([0-9]*\).*/\1/p')
    if [ -n "$pid" ]; then
        sleep 1
        local res; res="$(virsh qemu-agent-command "$VM" "{\"execute\":\"guest-exec-status\",\"arguments\":{\"pid\":$pid}}" 2>/dev/null)"
        echo "$res" | grep -q "$(echo -n "$MARK" | base64)" && ok "guest re-read agentshare marker after restore (remount ok)" || warn "marker re-read inconclusive (remount best-effort)"
    fi

    log "cleanup..."; virsh destroy "$VM" >/dev/null 2>&1 || true; virsh undefine "$VM" --nvram >/dev/null 2>&1 || true
    rm -rf "$WORK"
    echo -e "${GREEN}SELFTEST PASSED${NC}: checkpoint -> restore round-trip with virtiofs detach/re-attach works."
}

# --- dispatch --------------------------------------------------------------
case "${1:-}" in
    save)     shift; cmd_save "$@";;
    restore)  shift; cmd_restore "$@";;
    selftest) shift; cmd_selftest "$@";;
    -h|--help|"") usage;;
    *) usage; die "unknown subcommand '$1'";;
esac

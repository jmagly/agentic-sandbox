# [HIGH] virtiofs RW mounts missing nodev/nosuid/noexec → host attack via shared dir

**Labels**: `priority: high`, `area: security`, `area: virtiofs`, `type: incident`

## Summary

The fstab entries generated inside the VM by `images/qemu/cloud-init/ubuntu.sh:43-50` (and again at lines 393-400) for virtiofs mounts omit `nodev`, `nosuid`, and `noexec`:

```
agentglobal /mnt/global virtiofs ro,noatime,nofail 0 0
agentinbox  /mnt/inbox  virtiofs rw,noatime,nofail 0 0
agentoutbox /mnt/outbox virtiofs rw,noatime,nofail 0 0
```

The RW mounts (`agentinbox`, `agentoutbox`) are backed on the host by `$AGENTSHARE_ROOT/<vm>-inbox` and `<vm>-outbox`. A compromised agent inside the VM (running as root inside the guest by default) can:

1. Write a SUID-root binary onto `/mnt/inbox` — it persists with SUID bit set on the host filesystem.
2. Create device nodes via `mknod`.
3. Drop an executable that a host operator tab-completes into invocation while debugging.

## Remediation

1. In `cloud-init/ubuntu.sh`, both fstab-generation blocks: append `nodev,nosuid,noexec` to inbox and outbox lines.
2. Append `nodev,nosuid` to the global RO line. Skip `noexec` if global ships intentional helper binaries — document the choice.
3. Add a host-side guardrail in `setup-agentshare.sh`: a daily systemd-timer that runs `find $AGENTSHARE_ROOT -perm -4000 -o -perm -2000` and alerts if any SUID/SGID files appear.
4. Add to `validate-vm.sh` (per the existing VM-validation policy): post-provision, assert `/proc/mounts` shows the expected flags inside the VM.

## Acceptance

- New VMs provisioned after the fix show `nodev,nosuid,noexec` in `/proc/mounts` for `/mnt/inbox` and `/mnt/outbox`.
- Host-side `find` returns clean for existing inboxes.

## References

- Linux Capabilities Guide §4 (filesystem security)
- Internal audit finding H2 (secure-bootstrap-reviewer)

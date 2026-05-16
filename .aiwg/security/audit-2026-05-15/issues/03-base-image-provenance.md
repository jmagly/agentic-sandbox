# [BLOCK] Base ISO + qcow2 backing image have no signature/hash verification

**Labels**: `priority: critical`, `area: security`, `area: bootstrap`, `type: incident`

## Summary

The VM provisioning chain has no cryptographic anchor at its root.

- `images/qemu/build-base-image.sh:77-91` (`resolve_iso_path()`) accepts any file matching the version pattern in `$ISO_DIR` and feeds it directly to `virt-install`. No SHA256 check against `SHA256SUMS`, no GPG check of `SHA256SUMS` against the Ubuntu archive key.
- The resulting qcow2 is marked `chattr +i` (good) but no SHA256 is recorded.
- `images/qemu/provision-vm.sh:create_overlay_disk()` does not verify the backing file's hash before creating per-VM overlays.

An attacker with write access to `$ISO_DIR` or to `/mnt/ops/base-images/` (or who silently flips `chattr -i` once) owns every future agent VM.

## Required work

1. Create `images/qemu/iso-pins.json` with `{version, sha256, gpg_fingerprint}` for each supported Ubuntu version. Commit to repo; gate edits via code review.
2. In `build-base-image.sh`: fetch `SHA256SUMS` + `SHA256SUMS.gpg` for the target release; verify GPG against Ubuntu archive key `843938DF228D22F7B3742BC0D94AA3F0EFE21092`; then verify the local ISO matches the SHA256 from the now-trusted `SHA256SUMS`. Fail build on mismatch.
3. After `virt-install` + `virt-sparsify` completes, record `sha256sum /mnt/ops/base-images/ubuntu-server-${version}-agent.qcow2` to `/mnt/ops/base-images/manifest.json`.
4. In `provision-vm.sh:create_overlay_disk()`: read `manifest.json`, verify backing-file SHA256 matches recorded value, fail loudly on mismatch.
5. Document the rotation procedure for `iso-pins.json` when Ubuntu publishes a point release.

## Acceptance

- Provisioning a VM with a tampered base image fails before VM definition.
- `manifest.json` is present and verified on every `provision-vm.sh` invocation.
- CI: corrupt one byte of the base qcow2 → `provision-vm.sh` exits non-zero.

## References

- NIST SP 800-147 §3.1 (firmware/image authenticity)
- NIST SP 800-193 §3.2 (platform integrity verification)
- Reproducible Builds project — input pinning
- Internal audit finding B3 (secure-bootstrap-reviewer)

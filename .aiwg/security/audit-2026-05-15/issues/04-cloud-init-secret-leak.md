# [BLOCK] cloud-init.iso contains plaintext AGENT_SECRET in world-readable host path

**Labels**: `priority: critical`, `area: security`, `area: bootstrap`, `type: incident`

## Summary

Per-VM cloud-init ISOs at `/var/lib/agentic-sandbox/vms/<vm>/cloud-init.iso` contain the plaintext 256-bit `AGENT_SECRET`, plaintext health token, and ephemeral SSH public key (embedded in the cloud-init user-data section at `cloud-init/ubuntu.sh:117-125`).

The directory is created by `provision-vm.sh:545-547` with `sudo chown -R` to the invoking operator but no explicit mode tightening, so it inherits the operator's umask (typically 0755). Any local user on the host can:

```bash
mount -o loop /var/lib/agentic-sandbox/vms/<vm>/cloud-init.iso /mnt/x
grep AGENT_SECRET /mnt/x/user-data
```

The ISO is **not** detached after first boot — it stays attached as a readonly CDROM for the life of the domain. The cloud-init runtime copy at `/var/lib/cloud/instance/user-data.txt` inside the VM also retains a plaintext copy.

## Remediation

**Immediate hotfix** (30 minutes):
```bash
# in provision-vm.sh, after creating $vm_dir:
sudo chmod 700 "$vm_dir"
sudo chmod 600 "$cloud_init_iso"
```

Also tighten `SECRETS_DIR` in `images/qemu/lib/secrets.sh:25-28`: change `chmod 755` → `chmod 700`, change `chmod 644` on token files → `chmod 600`.

**Proper fix** (1-2 days): replace CD-ROM secret delivery with SSH-push. The ephemeral SSH key infrastructure is already in `lib/secrets.sh:174`. Boot the VM with `vendor-data` only (no secret); after SSH comes up, push `agent.env` over the wire; immediately `rm` the cloud-init.iso from host and `shred -u /var/lib/cloud/instance/user-data.txt` inside the VM via `runcmd`.

## Acceptance

- `find /var/lib/agentic-sandbox -type f -perm /o+r` returns nothing.
- `find /var/lib/agentic-sandbox/secrets -perm /o+r` returns nothing.
- Local-user `cat /var/lib/agentic-sandbox/vms/<vm>/cloud-init.iso` → Permission Denied.
- (proper fix only): cloud-init.iso does not contain `AGENT_SECRET` literal.

## References

- NIST SP 800-147 §2.4 (credential confidentiality in transit)
- Canonical cloud-init security docs
- Internal audit findings B4 + H3 (secure-bootstrap-reviewer, applied-cryptographer)

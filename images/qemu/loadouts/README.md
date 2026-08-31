# Loadout Manifests

Declarative YAML manifests for composable VM provisioning.

## Structure

```
loadouts/
  schema.yaml              # Full manifest schema reference
  resolve-manifest.sh      # Resolves extends chains into merged YAML
  generate-from-manifest.sh # Converts merged manifest to cloud-init user-data
  layers/                  # Composable building blocks
  providers/               # One per AIWG provider (9 total)
  profiles/                # Pre-built profiles ready to use
  tests/                   # Test suite
```

## Usage

```bash
# Provision a VM with a loadout
./provision-vm.sh agent-01 --loadout profiles/claude-only.yaml --start

# Debug: see the resolved manifest
./resolve-manifest.sh profiles/dual-review.yaml

# Debug: generate cloud-init without provisioning
TMPDIR=$(mktemp -d)
./resolve-manifest.sh profiles/claude-only.yaml > "$TMPDIR/resolved.yaml"
./generate-from-manifest.sh "$TMPDIR/resolved.yaml" test-vm "ssh-key" "$TMPDIR" \
    false "secret" "ephemeral-key" "mac" "" "token"
cat "$TMPDIR/user-data"
```

## Browser-QA Sessions

`profiles/browser-qa.yaml` provisions the carbonyl trusted-input stack and a private session mount:

- Host: `/var/lib/agentic-sandbox/vms/{vm}/carbonyl-sessions` with mode `0700`
- VM: `/home/agent/.local/share/carbonyl-agent/sessions`
- Mount tag: `carbonylsessions`

Cookie/session material should be mode `0600`. The sandbox only provides the mount; it does not import cookies.

The loadout intentionally does not download Carbonyl during cloud-init. After
provisioning, inject the exact feature-build artifact and its expected SHA-256,
then run acceptance checks:

```bash
./scripts/install-browser-qa-runtime.sh agent-browser /path/to/x86_64-unknown-linux-gnu.tgz <sha256>
./scripts/validate-browser-qa.sh agent-browser
```

The installer validates archive paths and verifies the digest both before and
after transfer. Display and input tests must run only against the VM's private
Xorg `:99`; do not expose host display sockets, `/dev/input`, `/dev/uinput`, or
`/dev/dri` to the guest.

## Runtime Options

Loadouts may declare `runtime_options` to describe portable launch intent for
management clients. The schema supports cold boot, snapshot/checkpoint restore,
fork-from-base, warm-pool handoff, required/excluded provider capabilities, and
the VFIO rule that GPU passthrough VMs must not use snapshot, restore, fork, or
warm-pool memory reuse. See `docs/LOADOUTS.md` for examples.

## Creating a custom profile

1. Create `profiles/my-profile.yaml`
2. Set `extends:` to compose layers
3. Override any values you need
4. Use with `--loadout profiles/my-profile.yaml`

See `docs/LOADOUTS.md` for full schema documentation.

## Running tests

```bash
cd images/qemu/loadouts
bash tests/test_generate_from_manifest.sh
```

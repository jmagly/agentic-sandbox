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

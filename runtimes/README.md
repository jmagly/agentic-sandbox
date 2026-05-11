# Runtime Definitions

Static runtime configuration files referenced by deployment scripts and documentation.

## Contents

- `qemu/ubuntu-agent.xml` — libvirt domain XML used by `scripts/sandbox-launch.sh` for the QEMU/KVM agent VM template
- `docker/docker-compose.yml` — docker-compose definition referenced by `docs/DEPLOYMENT.md` for container-based runtime profiles

## Status

These files are configuration assets, not code. They are loaded at deploy time and do not require regeneration. Modify them only when the corresponding runtime profile changes.

The primary VM provisioning flow lives in `../images/qemu/provision-vm.sh`, which generates fresh domain XML per-VM and does not use the template in `qemu/`. The template here is a fallback / reference for the older `scripts/sandbox-launch.sh` path.

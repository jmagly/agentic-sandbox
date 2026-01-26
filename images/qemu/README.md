# QEMU Base Image Build Infrastructure

Build agent-ready Ubuntu VM base images for the agentic-sandbox QEMU adapter.

## Quick Start

```bash
# Build Ubuntu 24.04 agent image
./build-base-image.sh 24.04

# Dry run to see what would happen
./build-base-image.sh --dry-run 24.04

# Build with custom disk size
./build-base-image.sh --disk-size 60G 24.04
```

## Requirements

### Host Packages

```bash
sudo apt install qemu-utils virtinst genisoimage libguestfs-tools
```

### ISOs

Place Ubuntu Server ISOs in `/mnt/ops/isos/linux/`:
- `ubuntu-22.04.x-live-server-amd64.iso`
- `ubuntu-24.04.x-live-server-amd64.iso`
- `ubuntu-25.10-live-server-amd64.iso`

### libvirt

```bash
sudo systemctl enable --now libvirtd
sudo usermod -aG libvirt $USER
```

## Output Images

Images are created in `/mnt/ops/base-images/`:
- `ubuntu-server-22.04-agent.qcow2`
- `ubuntu-server-24.04-agent.qcow2`
- `ubuntu-server-25.10-agent.qcow2`

## Image Configuration

All images include:

| Component | Purpose |
|-----------|---------|
| qemu-guest-agent | Enables `virsh qemu-agent-command` for exec |
| openssh-server | Fallback access method |
| cloud-init | First-boot configuration |
| python3 | Agent tooling |
| Common tools | curl, wget, git, jq, etc. |

### Default User

- **Username**: `agent`
- **Password**: Disabled (SSH key auth only)
- **Sudo**: Passwordless

## Creating Overlay VMs

Use the base image as a backing file for copy-on-write VMs:

```bash
# Create overlay disk
qemu-img create -f qcow2 \
  -b /mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2 \
  -F qcow2 \
  /path/to/agent-01.qcow2 20G

# Check overlay info
qemu-img info /path/to/agent-01.qcow2
```

## Customization

### Autoinstall Templates

- `autoinstall/user-data.template` - Ubuntu autoinstall configuration
- `autoinstall/meta-data.template` - Instance metadata

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ISO_DIR` | `/mnt/ops/isos/linux` | Directory containing ISOs |
| `BASE_DIR` | `/mnt/ops/base-images` | Output directory for images |

## Troubleshooting

### Monitor Installation

```bash
virsh console build-agent-24.04
```

### Check VM State

```bash
virsh list --all
virsh domstate build-agent-24.04
```

### View Logs

```bash
virsh dumpxml build-agent-24.04 | grep -A5 console
```

### Clean Up Failed Build

```bash
virsh destroy build-agent-24.04
virsh undefine build-agent-24.04 --nvram
rm /mnt/ops/base-images/ubuntu-server-24.04-agent.qcow2
```

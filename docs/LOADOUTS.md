# Loadout Manifest System

Declarative YAML manifests for composable VM provisioning. Loadouts define what tools, runtimes, AI providers, and AIWG frameworks get installed in a VM.

## Quick Start

```bash
# Single-provider VM with Claude Code
./provision-vm.sh agent-01 --loadout profiles/claude-only.yaml --agentshare --start

# Two providers for cross-checking
./provision-vm.sh agent-02 --loadout profiles/dual-review.yaml --start

# Isolated security audit environment
./provision-vm.sh agent-03 --loadout profiles/security-audit.yaml --start

# Full suite - all 9 providers, all 6 frameworks
./provision-vm.sh agent-04 --loadout profiles/full-suite.yaml --cpus 8 --memory 32G --start
```

## Pre-Built Profiles

### Per-Provider

| Profile | AI Tools | AIWG Framework | Resources |
|---------|----------|---------------|-----------|
| `claude-only` | Claude Code | sdlc-complete -> claude-code | 4 CPU, 8G |
| `codex-only` | Codex CLI | sdlc-complete -> codex | 4 CPU, 8G |
| `copilot-only` | Copilot CLI | sdlc-complete -> copilot | 4 CPU, 8G |

### Collaboration

| Profile | AI Tools | AIWG Framework | Resources |
|---------|----------|---------------|-----------|
| `dual-review` | Claude + Codex | sdlc -> [claude-code, codex] | 4 CPU, 12G |
| `multi-provider` | Claude + Codex + Copilot | sdlc -> [claude-code, codex, copilot] | 6 CPU, 16G |
| `full-suite` | All 4 tools | All 6 frameworks -> all 9 providers | 8 CPU, 32G |

### Task-Focused

| Profile | Purpose | Network | AIWG Framework |
|---------|---------|---------|---------------|
| `security-audit` | Forensics/security analysis | **isolated** | forensics-complete |
| `research-station` | Deep research tasks | full | research-complete |
| `sdlc-team` | Collaborative SDLC development | full | sdlc + ops |

### Backward Compatibility

| Profile | Equivalent To |
|---------|--------------|
| `basic` | `--profile basic` |
| `agentic-dev` | `--profile agentic-dev` |

## Manifest Schema

```yaml
apiVersion: loadout/v1
kind: loadout              # "loadout" (complete) or "layer" (composable partial)

metadata:
  name: my-profile
  description: What this profile does
  labels:
    category: per-provider   # per-provider | collaboration | task-focused

extends:                   # Composable inheritance (depth-first, left-to-right)
  - layers/base-dev.yaml
  - layers/docker.yaml
  - providers/claude-code.yaml

resources:
  cpus: 4
  memory: 8G
  disk: 40G
  gpu:
    enabled: false
    device: "0000:01:00.0"  # PCI device ID for passthrough

network:
  mode: full               # isolated | allowlist | full

packages:                  # apt packages
  - ripgrep
  - jq

runtimes:
  python:
    enabled: true
    method: uv             # uv (default) | system
    tools: [ruff, aider-chat]
  node:
    enabled: true
    method: fnm            # fnm (default) | system
    version: lts
    package_manager: pnpm
    global_packages: [aiwg, "@openai/codex"]
  go:
    enabled: true
    version: latest
    tools: [github.com/fullstorydev/grpcurl/cmd/grpcurl@latest]
  rust:
    enabled: true
    components: [clippy, rustfmt, rust-analyzer]
    crates: [xh, websocat, hyperfine]
  bun:
    enabled: true

ai_tools:
  claude_code:
    enabled: true
    channel: stable
    settings: { model: claude-sonnet-4-5-20250929 }
  aider:
    enabled: true
    config: { model: claude-3-5-sonnet-20241022, auto_commits: false }
  codex:
    enabled: true
    config: { model: gpt-4o, approval_mode: suggest }
  copilot:
    enabled: true

aiwg:
  enabled: true
  frameworks:
    - name: sdlc-complete
      providers: [claude-code, codex]

docker:
  enabled: true
  mode: rootless
```

## Composable Layers

Manifests can extend other manifests via `extends:`. Resolution is depth-first, left-to-right:

- **Scalars**: last value wins (most-specific manifest)
- **String arrays**: union + dedup (e.g., packages merge)
- **Object arrays**: concatenate (e.g., frameworks append)
- **Maps**: deep merge (recursive)

### Available Layers

| Layer | Contents |
|-------|----------|
| `layers/base-minimal.yaml` | SSH, health server, UFW, qemu-guest-agent |
| `layers/base-dev.yaml` | Languages, build tools, CLI tools (extends base-minimal) |
| `layers/ai-tools.yaml` | Claude Code, Aider, Codex CLI |
| `layers/docker.yaml` | Rootless Docker with compose + buildx |
| `layers/databases.yaml` | PostgreSQL, MySQL, Redis, SQLite clients |
| `layers/observability.yaml` | strace, sysstat, iotop, nethogs |
| `layers/network-tools.yaml` | xh, grpcurl, websocat, hyperfine |

### Provider Layers

One per AIWG provider. Each declares prerequisites and AI tool config:

| Provider | AI Tool | Prerequisites |
|----------|---------|---------------|
| `providers/claude-code.yaml` | Claude Code CLI | Node.js |
| `providers/codex.yaml` | @openai/codex | Node.js |
| `providers/copilot.yaml` | GitHub Copilot CLI | Node.js |
| `providers/factory.yaml` | (framework only) | Node.js |
| `providers/cursor.yaml` | (framework only) | Node.js |
| `providers/opencode.yaml` | (framework only) | Node.js |
| `providers/warp.yaml` | (framework only) | Node.js |
| `providers/windsurf.yaml` | (framework only) | Node.js |
| `providers/openclaw.yaml` | (framework only) | Node.js |

## Creating Custom Profiles

1. Create a YAML file in `images/qemu/loadouts/profiles/`:

```yaml
apiVersion: loadout/v1
kind: loadout
metadata:
  name: my-custom
  description: Custom profile for my use case

extends:
  - layers/base-dev.yaml
  - layers/docker.yaml
  - providers/claude-code.yaml
  - providers/codex.yaml

resources:
  memory: 16G

aiwg:
  enabled: true
  frameworks:
    - name: sdlc-complete
      providers: [claude-code, codex]
```

2. Use it:
```bash
./provision-vm.sh agent-01 --loadout profiles/my-custom.yaml --start
```

## CLI Override Precedence

CLI flags always override manifest values:

```bash
# Manifest says 8G memory, but CLI overrides to 16G
./provision-vm.sh agent-01 --loadout profiles/claude-only.yaml --memory 16G
```

## Network Modes

| Mode | Behavior |
|------|----------|
| `full` | Unrestricted egress (default) |
| `allowlist` | DNS-filtered, HTTPS-only (requires Blocky) |
| `isolated` | Management server only, no internet |

## GPU Passthrough

VMs can access host GPUs for ML inference, security testing, and other GPU workloads:

```yaml
resources:
  gpu:
    enabled: true
    device: "0000:01:00.0"   # PCI device ID (lspci -nn)
    driver: vfio-pci          # default
```

### Prerequisites

1. Host IOMMU enabled (`intel_iommu=on` or `amd_iommu=on` in kernel cmdline)
2. GPU bound to `vfio-pci` driver on the host
3. PCI device ID from `lspci -nn` (e.g., `0000:01:00.0`)

### What Happens

- The loadout generator writes a `gpu-config` sidecar file
- `provision-vm.sh` adds a `<hostdev>` PCI passthrough element to the libvirt XML
- Cloud-init installs GPU drivers via `ubuntu-drivers install --gpgpu`
- The GPU is exclusively owned by the VM (not shared with host)

### Example

```bash
# Security audit with GPU for accelerated hash cracking
./provision-vm.sh agent-01 --loadout profiles/security-audit.yaml --start
# (edit security-audit.yaml to set resources.gpu.enabled: true and device)
```

## AIWG Provider Matrix

| Provider | Native Features | Emulated Features |
|----------|----------------|-------------------|
| claude-code | cron, agent_teams, tasks, MCP | behaviors, mission_control |
| codex | (none) | all via aiwg-mc |
| copilot | (none) | all via aiwg-mc |
| factory | (none) | all via aiwg-mc |
| cursor | (none) | all via aiwg-mc |
| opencode | (none) | all via aiwg-mc |
| warp | (none) | all via aiwg-mc |
| windsurf | (none) | all via aiwg-mc |
| openclaw | MCP, behaviors | cron, tasks via aiwg-mc |

## AIWG Frameworks

| Framework | Purpose |
|-----------|---------|
| sdlc-complete | Software development lifecycle (58 agents, 42+ commands) |
| ops-complete | Operations and infrastructure |
| forensics-complete | Digital forensics and incident response |
| research-complete | Research workflow automation |
| media-curator | Media archive management |
| media-marketing-kit | Marketing content toolkit |

## Directory Structure

```
images/qemu/loadouts/
  schema.yaml              # Manifest schema reference
  resolve-manifest.sh      # YAML inheritance resolver
  generate-from-manifest.sh # Manifest -> cloud-init generator
  layers/                  # Composable base layers
  providers/               # Per-AIWG-provider layers
  profiles/                # Pre-built composed profiles
  tests/                   # Test suite
```

## Troubleshooting

### Manifest not found
```bash
# Paths are relative to images/qemu/loadouts/
./provision-vm.sh agent-01 --loadout profiles/claude-only.yaml  # correct
./provision-vm.sh agent-01 --loadout /absolute/path/to/manifest.yaml  # also works
```

### Package conflicts
The resolver deduplicates string arrays. If two layers specify the same package, it appears once.

### Debugging resolution
```bash
# See the fully resolved manifest
cd images/qemu/loadouts
./resolve-manifest.sh profiles/full-suite.yaml
```

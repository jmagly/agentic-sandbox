# Getting Started

From a fresh `git clone` to a running agent in ~15 minutes (longer if the first Rust build is cold).

This guide walks you through the **single fastest path**: start the management server, attach via the dashboard, run a container-runtime agent. Once that works, you can graduate to full KVM VMs ([VM path](#3-vm-path-full-isolation) below) or skip the dashboard entirely ([direct CLI path](#4-direct-cli-path)).

> **Already know what you want?**
> - **Container agent in 2 minutes** -> [Quick path: container runtime](#2-quick-path-container-runtime)
> - **Full KVM-isolated VM** -> [VM path: full isolation](#3-vm-path-full-isolation)
> - **No dashboard, scripted** -> [Direct CLI path](#4-direct-cli-path)
> - **Integrate with `aiwg serve`** → [AIWG Executor docs](aiwg-executor.md)

---

## 0. Verify prerequisites

Run this one-liner to check everything at once. It's read-only and won't change anything:

```bash
echo "KVM:      $(egrep -c '(vmx|svm)' /proc/cpuinfo 2>/dev/null) (need >0 for VM runtime)" && \
echo "libvirt:  $(systemctl is-active libvirtd 2>/dev/null || echo missing)" && \
echo "Docker:   $(docker info >/dev/null 2>&1 && echo running || echo missing)" && \
echo "Rust:     $(rustc --version 2>/dev/null || echo missing)" && \
echo "protoc:   $(protoc --version 2>/dev/null || echo missing)" && \
echo "make:     $(make --version 2>/dev/null | head -1 || echo missing)"
```

Expected for the **container path** (fastest first run):

- Rust 1.75+, protoc, make, Docker running

Expected for the **VM path** (full isolation):

- All of the above **plus** KVM count > 0 and `libvirtd active`

If any are missing, install them:

```bash
# Ubuntu/Debian
sudo apt update && sudo apt install -y \
    qemu-kvm libvirt-daemon-system protobuf-compiler build-essential
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add yourself to libvirt + kvm groups (for VM runtime)
sudo usermod -aG libvirt,kvm "$USER"
# Log out and back in for group membership to apply

# Docker (skip if not using container runtime)
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker "$USER"
```

---

## 1. Clone and build

```bash
git clone https://github.com/jmagly/agentic-sandbox.git
cd agentic-sandbox
make build         # builds all three crates: management, agent-rs, cli
```

**First-build time**: 10–25 minutes on cold caches (Rust + many crates). Subsequent builds are cached.

You'll get three release binaries:

- `management/target/release/agentic-mgmt` — the control plane
- `cli/target/release/sandboxctl` — the CLI (also aliased `agentic-sandbox`)
- `agent-rs/target/release/agent-client` — the in-VM/in-container agent

Optional: symlink the CLI onto your PATH:

```bash
ln -sf "$(pwd)/cli/target/release/sandboxctl" ~/.local/bin/
```

---

## 2. Quick path: container runtime

The container path is the **fastest way to verify your install works**. Container instances start in seconds and don't need KVM.

```bash
# Start the management server (foreground; use a second terminal or screen/tmux)
cd management && ./dev.sh

# In a second terminal:
# Open the dashboard
xdg-open http://localhost:8122   # or just open this URL in any browser
```

In the dashboard:

1. Click **+ Create Instance** (top of sidebar).
2. **Runtime**: select **Container**.
3. **Image**: pick `agentic/claude:latest` (or `codex`, `opencode`).
4. **Name**: anything matching `[a-z0-9-]+` (e.g. `agent-01`).
5. Click **Create**. The instance appears in the sidebar within ~2 seconds.
6. Click the row → click **📺 Pane** to attach a live terminal.

You now have a sandboxed agent process. The dashboard shows live PTY output, lets you submit tasks, and reports HITL prompts.

### Same flow from the CLI

```bash
sandboxctl config set-context local --server http://localhost:8122
sandboxctl container create agent-01 --image agentic/claude:latest
sandboxctl agent list
sandboxctl session list --agent agent-01
sandboxctl session attach <session-id> --write   # Ctrl-A d to detach
```

---

## 3. VM path: full isolation

If you want hardware-level isolation (each agent gets its own kernel), use the VM runtime.

**Additional prerequisite**: an Ubuntu 24.04 base image. Build it once:

```bash
cd images/qemu
./build-base-image.sh 24.04          # ~5–15 min, depending on network speed
```

This downloads the Ubuntu cloud image, verifies its checksum, and stages it for fast cloning.

Then from the dashboard:

1. **+ Create Instance** → **Runtime: VM** → pick a loadout (`claude-only`, `dual-review`, `full-suite`).
2. **Create**. VM provision time: 30 s – 10 min depending on loadout (loadouts that install Claude Code / Codex CLI take longer).
3. The VM appears with a `[VM]` badge. Attach via the Pane button or SSH:

   ```bash
   ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<vm-ip>
   ```

The VM agent connects back to the management server automatically on boot.

### Same flow from the CLI

```bash
sandboxctl vm create agent-02 --loadout profiles/claude-only.yaml --agentshare --start
```

See [LOADOUTS.md](LOADOUTS.md) for the full loadout reference and [container-runtime.md](container-runtime.md) for the container variant.

---

## 4. Direct CLI path

Want to provision a single VM without the management server? The provisioner runs standalone:

```bash
./images/qemu/provision-vm.sh agent-01 \
    --loadout profiles/claude-only.yaml \
    --agentshare \
    --start

# Agent inside the VM will try to dial host.internal:8120; if no server
# is running, the VM is still SSH-reachable as an isolated environment.
ssh -i /var/lib/agentic-sandbox/secrets/ssh-keys/agent-01 agent@<vm-ip>
```

Useful flags: `--profile basic`, `--cpus 8 --memory 16G --disk 100G`, `--network-mode isolated|allowlist|full`. Full reference: [images/qemu/README.md](https://github.com/jmagly/agentic-sandbox/blob/main/images/qemu/README.md).

---

## 5. Submit your first task

Once an agent is running, submit a task via the dashboard, the CLI, or REST.

### Dashboard

Click your agent → **Tasks** tab → **+ New Task** → paste a prompt → **Submit**.

### CLI

```bash
cat > task.yaml <<'EOF'
version: "1"
kind: Task
metadata:
  id: ""
  name: "Workspace summary"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "List the files in /workspace and summarize what you see."
  model: "claude-sonnet-4-5-20250929"
lifecycle:
  timeout: "5m"
EOF
sandboxctl task submit --file task.yaml --wait
```

### REST

```bash
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "manifest": {
      "version": "1",
      "kind": "Task",
      "metadata": {
        "id": "",
        "name": "Workspace summary"
      },
      "repository": {
        "url": "https://github.com/example/repo.git",
        "branch": "main"
      },
      "claude": {
        "prompt": "List the files in /workspace and summarize what you see.",
        "model": "claude-sonnet-4-5-20250929"
      },
      "lifecycle": {
        "timeout": "5m"
      }
    }
  }'
```

Task lifecycle, HITL prompts, and event streaming are covered in [task-orchestration-api.md](task-orchestration-api.md).

---

## 6. What's next?

You have a working install. Pick the path that matches what you want to do next:

| If you want to… | Go to… |
|---|---|
| Understand the surfaces (admin / A2A / observability) | [concepts.md](concepts.md), [v2-migration-guide.md](v2-migration-guide.md) |
| Run AIWG missions on this executor | [aiwg-executor.md](aiwg-executor.md) |
| Tune VM/container resource limits | [DEPLOYMENT.md](#/DEPLOYMENT), [OPERATIONS.md](#/OPERATIONS) |
| Build custom loadouts | [LOADOUTS.md](#/LOADOUTS) |
| Hook the dashboard into monitoring | [monitoring.md](monitoring.md), [observability/](observability/) |
| Troubleshoot a stuck install | [TROUBLESHOOTING.md](#/TROUBLESHOOTING), [crash-loop.md](crash-loop.md) |
| Understand the API surface in depth | [API.md](#/API), [ws-protocol.md](ws-protocol.md) |
| See the full architecture | [ARCHITECTURE.md](#/ARCHITECTURE), [ECOSYSTEM.md](#/ECOSYSTEM) |

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `./dev.sh` fails with "binary not found" | `make build` not run yet | Run `make build` from repo root first |
| Dashboard loads but instance creation fails | Docker not running (container path) or libvirtd not running (VM path) | `systemctl start docker` or `systemctl start libvirtd` |
| VM provision hangs at "waiting for cloud-init" | First boot is slow; SSH not yet ready | Wait ~60 s, then retry; check `virsh console <vm-name>` |
| "Invalid agent secret" in agent logs | Token mismatch between host and VM | Use `./scripts/deploy-agent.sh <vm>` to redeploy; never edit secrets manually |
| Browser shows "connection refused" | Management server not listening on 8122 | `cd management && ./dev.sh logs` to see startup errors |
| `protoc` missing during build | Protocol Buffers compiler not installed | `sudo apt install protobuf-compiler` or `brew install protobuf` |

For anything not in this table, check [TROUBLESHOOTING.md](#/TROUBLESHOOTING) or open an issue at <https://github.com/jmagly/agentic-sandbox/issues>.

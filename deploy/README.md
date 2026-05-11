# deploy — Deployment Artifacts

Production-grade artifacts for running agentic-sandbox: cloud-init seeds for VMs, systemd units, container images and compose stacks, and the agent install script. These are consumed by [`images/qemu/provision-vm.sh`](../images/qemu/provision-vm.sh), by host operators, and by CI.

## Layout

| Path                                              | Purpose                                                                                                                     |
|---------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------|
| `agent.env.template`                              | Reference environment file for an agent VM. Copied to `/etc/agentic-sandbox/agent.env` (root:root, mode 0600) at provision. |
| `install-agent.sh`                                | Installer script (`rust`, `python`, or `both`) for adding the agent client to an existing VM. Handles systemd unit drop-in and `agent.env` scaffold. |
| `cloud-init/user-data.template`                   | Cloud-init `#cloud-config` template. Carries `{{AGENT_ID}}`, `{{AGENT_SECRET}}`, `{{MANAGEMENT_SERVER}}`, `{{AGENT_VARIANT}}` placeholders. `provision-vm.sh` substitutes and seeds it into the cidata ISO. |
| `systemd/agent-client.service`                    | Hardened systemd unit for the Rust agent. `Type=simple`, `Restart=always`, `NoNewPrivileges`, `ProtectSystem=strict`, `ReadWritePaths=/mnt/inbox`, `MemoryMax=512M`, `CPUQuota=200%`. |
| `systemd/agent-client-python.service`             | Reference unit for the (legacy) Python agent variant.                                                                       |
| `docker/Dockerfile.agent-rust`                    | Multi-stage build for the Rust agent client. Stage 1: `rust:1.88-bookworm` + protoc, `cargo build --release --locked`. Stage 2: `debian:bookworm-slim` runtime. |
| `docker/Dockerfile.agent-python`                  | Multi-stage build for the Python agent variant.                                                                             |
| `docker/Dockerfile.management`                    | Multi-stage build for the management server. Stage 1 needs `libvirt-dev` and `pkg-config` for the libvirt bindings.         |
| `docker/docker-compose.production.yaml`           | Production compose: management server only (agents run in VMs, connect back over 8120). Image pulled from the Gitea registry. Health-checked, log-rotated. |
| `docker/docker-compose.agents.yaml`               | Developer compose: management + N containerized agents on a shared bridge network. For local validation of the agent-to-server protocol without QEMU. |

## Relationship to Provisioning

The end-to-end flow:

1. Operator runs `images/qemu/provision-vm.sh agent-01 --profile agentic-dev --agentshare --start`.
2. The script generates a 64-hex-char agent secret, writes the SHA-256 hash to `~/.config/agentic-sandbox/agent-tokens` on the host, and stages the plaintext for cloud-init only.
3. The script substitutes placeholders in [`cloud-init/user-data.template`](cloud-init/user-data.template) and emits a cidata ISO.
4. The VM boots; cloud-init creates `/etc/agentic-sandbox/agent.env` (mode 0600) with the plaintext secret, installs the agent binary (from `/mnt/global/bin/` via agentshare or from the install script), drops the systemd unit, and enables `agent-client.service`.
5. The agent opens its gRPC `Connect()` stream to `MANAGEMENT_SERVER`; the management server validates the bearer against the stored hash and acks registration.

The plaintext secret only exists on the VM. The host stores only the hash. See [`../docs/security/resource-quota-design.md`](../docs/security/resource-quota-design.md) and the security model summary in [`../docs/welcome.md`](../docs/welcome.md).

## agent.env Variables

| Variable               | Required | Description                                                                                |
|------------------------|----------|--------------------------------------------------------------------------------------------|
| `AGENT_ID`             | yes      | Unique agent identifier. Must match the VM name and the host's hash-table key.            |
| `AGENT_SECRET`         | yes      | 64-hex-char shared secret. Plaintext on VM only; host stores SHA-256.                     |
| `MANAGEMENT_SERVER`    | yes      | `host:port` of the management gRPC endpoint. Default `host.internal:8120`. From the VM, this resolves to the libvirt bridge IP (usually `192.168.122.1:8120`). |
| `HEARTBEAT_INTERVAL`   | no       | Seconds between heartbeats. Default 30.                                                   |
| `AGENT_PROFILE`        | no       | Provision profile name (`basic`, `agentic-dev`). Reported in registration; used for labels. |

## install-agent.sh Usage

For attaching the agent to a VM that wasn't provisioned by `provision-vm.sh` (e.g., a manually-installed Debian/Ubuntu host):

```bash
sudo ./deploy/install-agent.sh rust \
  --agent-id agent-99 \
  --secret "$(openssl rand -hex 32)" \
  --server 192.168.122.1:8120
```

The script writes the systemd unit, populates `/etc/agentic-sandbox/agent.env`, and enables the service. The secret hash still has to be recorded on the host's `agent-tokens` file by hand for this code path.

## Docker / Compose Notes

The production compose (`docker-compose.production.yaml`) is intended for hosts that want to run only the management server in a container, with agents in real KVM VMs. The dev compose (`docker-compose.agents.yaml`) is for full-stack validation locally.

Both stacks listen on the same three ports (8120 gRPC, 8121 WebSocket, 8122 HTTP). The production stack binds to `127.0.0.1` and expects a reverse proxy (nginx, Caddy) in front for TLS.

Images are pulled from the Gitea registry at `git.integrolabs.net/roctinam/agentic-sandbox/`.

## See Also

- [`../images/qemu/provision-vm.sh`](../images/qemu/provision-vm.sh) — primary entry point for VM provisioning
- [`../docs/DEPLOYMENT.md`](../docs/DEPLOYMENT.md) — operator-facing deployment guide
- [`../docs/OPERATIONS.md`](../docs/OPERATIONS.md) — day-2 ops
- [`../docs/platform-support.md`](../docs/platform-support.md) — supported runtimes and roadmap
- [`../scripts/deploy-agent.sh`](../scripts/deploy-agent.sh) — push a freshly-built agent binary into a running VM
- [`../scripts/dev-deploy-all.sh`](../scripts/dev-deploy-all.sh) — full rebuild + deploy to every running VM

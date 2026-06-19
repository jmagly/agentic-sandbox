# deploy — Deployment Artifacts

Production-grade artifacts for running agentic-sandbox: cloud-init seeds for VMs, systemd units, container images and compose stacks, and the agent install script. These are consumed by [`images/qemu/provision-vm.sh`](../images/qemu/provision-vm.sh), by host operators, and by CI.

## Layout

| Path                                              | Purpose                                                                                                                     |
|---------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------|
| `agent.env.template`                              | Reference environment file for an agent VM. Copied to `/etc/agentic-sandbox/agent.env` (root:root, mode 0600) at provision. |
| `install-agent.sh`                                | Installer script (`rust`, `python`, or `both`) for adding the agent client to an existing VM. Handles systemd unit drop-in and `agent.env` scaffold. |
| `cloud-init/user-data.template`                   | Cloud-init `#cloud-config` template. Carries `{{AGENT_ID}}`, `{{MANAGEMENT_SERVER}}`, optional `{{AGENT_TRANSPORT_ENV}}`, and `{{AGENT_VARIANT}}` placeholders. `provision-vm.sh` substitutes and seeds it into the cidata ISO. |
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
2. The script requires secure transport provisioning. It stages mTLS client material and writes `AGENT_TRANSPORT=auto` plus `AGENT_GRPC_TLS_*` paths into cloud-init, or issues bootstrap enrollment material for first-start mTLS enrollment.
3. Legacy TCP `AGENT_SECRET` provisioning is retired and fails closed.
4. The script substitutes placeholders in [`cloud-init/user-data.template`](cloud-init/user-data.template) and emits a cidata ISO.
5. The VM boots; cloud-init creates `/etc/agentic-sandbox/agent.env` (mode 0600), installs the agent binary (from `/mnt/global/bin/` via agentshare or from the install script), drops the systemd unit, and enables `agent-client.service`.
6. The agent opens its gRPC `Connect()` stream to `MANAGEMENT_SERVER` and authenticates with transport identity material.

See [`../docs/security/resource-quota-design.md`](../docs/security/resource-quota-design.md) and the security model summary in [`../docs/welcome.md`](../docs/welcome.md).

## agent.env Variables

| Variable               | Required | Description                                                                                |
|------------------------|----------|--------------------------------------------------------------------------------------------|
| `AGENT_ID`             | yes      | Unique agent identifier.                                                                  |
| `MANAGEMENT_SERVER`    | yes      | `host:port` of the management gRPC endpoint. Default `host.internal:8120`. From the VM, this resolves to the libvirt bridge IP (usually `192.168.122.1:8120`). |
| `AGENT_TRANSPORT`      | secure   | Transport mode for the Rust agent. New secure provisions use `auto`.                     |
| `AGENT_GRPC_TLS_CA`    | secure   | Guest path to the gRPC mTLS CA bundle. Required with the other `AGENT_GRPC_TLS_*` variables for secure provisioning. |
| `AGENT_GRPC_TLS_CERT`  | secure   | Guest path to the gRPC mTLS client certificate.                                           |
| `AGENT_GRPC_TLS_KEY`   | secure   | Guest path to the gRPC mTLS client private key.                                           |
| `AGENT_GRPC_TLS_SERVER_NAME` | no | Expected gRPC mTLS server name. Defaults to `host.internal` in QEMU provisioning.          |
| `AGENT_BOOTSTRAP_TOKEN` | secure  | One-time bootstrap enrollment token used only for first-start mTLS certificate enrollment. |
| `AGENT_BOOTSTRAP_SPIFFE_ID` | secure | SPIFFE URI bound to the one-time bootstrap token. Required with `AGENT_BOOTSTRAP_TOKEN`. |
| `AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS` | no | Expiry timestamp for operator diagnostics. |
| `AGENT_BOOTSTRAP_TLS_DIR` | no | Directory where the Rust agent writes enrolled mTLS files. |
| `AGENT_BOOTSTRAP_ENROLLMENT_URL` | no | Explicit HTTP enrollment endpoint when the agent cannot derive it from `MANAGEMENT_SERVER`. |
| `HEARTBEAT_INTERVAL`   | no       | Seconds between heartbeats. Default 30.                                                   |
| `AGENT_PROFILE`        | no       | Provision profile name (`basic`, `agentic-dev`). Reported in registration; used for labels. |

`scripts/provision-vm-agent.sh` validates `/etc/agentic-sandbox/agent.env`
before installing or restarting the service. It fails closed if the VM still
has retired `AGENT_SECRET` material or lacks bootstrap enrollment, mTLS, UDS, or
vsock transport configuration.

## install-agent.sh Usage

For attaching the agent to a VM that wasn't provisioned by `provision-vm.sh`
(e.g., a manually-installed Debian/Ubuntu host), place the mTLS files on the
guest first:

```bash
sudo ./deploy/install-agent.sh rust \
  --agent-id agent-99 \
  --server 192.168.122.1:8120 \
  --transport auto \
  --tls-ca /etc/agentic-sandbox/grpc-mtls/ca.pem \
  --tls-cert /etc/agentic-sandbox/grpc-mtls/agent.pem \
  --tls-key /etc/agentic-sandbox/grpc-mtls/agent-key.pem \
  --tls-server-name host.internal
```

The script writes the systemd unit, populates `/etc/agentic-sandbox/agent.env`,
and enables the service. Legacy `--secret` input is rejected.

## Docker / Compose Notes

The production compose (`docker-compose.production.yaml`) is intended for hosts that want to run only the management server in a container, with agents in real KVM VMs. The dev compose (`docker-compose.agents.yaml`) is for full-stack validation locally.

Both stacks listen on the same three ports (8120 gRPC, 8121 WebSocket, 8122 HTTP). The production stack binds to `127.0.0.1` and expects a reverse proxy (nginx, Caddy) in front for TLS.

Images are pulled from the configured container registry. Public examples use
`registry.example.invalid/agentic-sandbox/` placeholders; operators should
substitute their deployment registry.

## See Also

- [`../images/qemu/provision-vm.sh`](../images/qemu/provision-vm.sh) — primary entry point for VM provisioning
- [`../docs/DEPLOYMENT.md`](../docs/DEPLOYMENT.md) — operator-facing deployment guide
- [`../docs/OPERATIONS.md`](../docs/OPERATIONS.md) — day-2 ops
- [`../docs/platform-support.md`](../docs/platform-support.md) — supported runtimes and roadmap
- [`../scripts/deploy-agent.sh`](../scripts/deploy-agent.sh) — push a freshly-built agent binary into a running VM
- [`../scripts/dev-deploy-all.sh`](../scripts/dev-deploy-all.sh) — full rebuild + deploy to every running VM

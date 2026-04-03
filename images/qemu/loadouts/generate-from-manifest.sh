#!/bin/bash
# generate-from-manifest.sh - Generate cloud-init user-data from a resolved loadout manifest
#
# Reads a resolved (already merged) loadout manifest YAML and generates a complete
# cloud-init user-data file by delegating the heavy lifting to an inline Python script.
#
# Usage:
#   ./generate-from-manifest.sh <manifest.yaml> <vm_name> <ssh_pubkey> <output_dir> \
#       <use_agentshare> <agent_secret> <ephemeral_ssh_pubkey> <mac_address> \
#       <network_mode> <health_token> [management_server]
#
# Arguments:
#   $1  resolved manifest YAML path (output of resolve-manifest.sh)
#   $2  VM name (e.g. agent-01)
#   $3  user SSH public key content (the key text, not a file path)
#   $4  output directory (user-data written here)
#   $5  use_agentshare: true|false
#   $6  agent_secret: 256-bit hex string
#   $7  ephemeral SSH public key content
#   $8  MAC address (e.g. 52:54:00:ab:cd:ef)
#   $9  network_mode: isolated|allowlist|full  (overrides manifest if non-empty)
#   $10 health_token
#   $11 management_server (default: host.internal:8120)
#
# Output:
#   $output_dir/user-data  — valid cloud-init #cloud-config YAML

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() { echo "error: $1" >&2; exit 1; }

# ── argument validation ────────────────────────────────────────────────────────
[[ $# -ge 10 ]] || die "usage: $0 manifest vm_name ssh_pubkey output_dir use_agentshare agent_secret ephemeral_pubkey mac network_mode health_token [management_server]"

MANIFEST="$1"
VM_NAME="$2"
SSH_PUBKEY_ARG="$3"
# $3 may be a file path or key content — read file if it exists
if [[ -f "$SSH_PUBKEY_ARG" ]]; then
    SSH_PUBKEY=$(cat "$SSH_PUBKEY_ARG")
else
    SSH_PUBKEY="$SSH_PUBKEY_ARG"
fi
OUTPUT_DIR="$4"
USE_AGENTSHARE="$5"
AGENT_SECRET="$6"
EPHEMERAL_PUBKEY="$7"
MAC_ADDRESS="$8"
NETWORK_MODE_ARG="$9"
HEALTH_TOKEN="${10}"
MANAGEMENT_SERVER="${11:-host.internal:8120}"

[[ -f "$MANIFEST" ]] || die "manifest not found: $MANIFEST"
[[ -d "$OUTPUT_DIR" ]] || die "output directory not found: $OUTPUT_DIR"

# ── Python generator ───────────────────────────────────────────────────────────
# The Python script reads the manifest and emits cloud-init user-data with
# placeholders, which are then replaced by sed below.

python3 - "$MANIFEST" "$USE_AGENTSHARE" "$NETWORK_MODE_ARG" "$MANAGEMENT_SERVER" \
    "$OUTPUT_DIR/user-data" <<'PYTHON_EOF'
import sys
import yaml
import json
import textwrap

manifest_path  = sys.argv[1]
use_agentshare = sys.argv[2].lower() == "true"
network_mode_arg = sys.argv[3]   # may be empty string; overrides manifest if set
management_server = sys.argv[4]
output_path    = sys.argv[5]

# ── load manifest ──────────────────────────────────────────────────────────────
with open(manifest_path) as f:
    m = yaml.safe_load(f) or {}

# ── helpers ────────────────────────────────────────────────────────────────────
def get(path, default=None):
    """Dot-separated key lookup into the manifest dict."""
    parts = path.split(".")
    cur = m
    for p in parts:
        if not isinstance(cur, dict):
            return default
        cur = cur.get(p)
        if cur is None:
            return default
    return cur

def enabled(path):
    return bool(get(path + ".enabled", False))

packages_list = get("packages", [])

# Effective network mode: CLI arg overrides manifest
net_mode = network_mode_arg if network_mode_arg else get("network.mode", "full")

# ── feature flags ──────────────────────────────────────────────────────────────
has_docker      = enabled("docker")
has_python      = enabled("runtimes.python")
has_node        = enabled("runtimes.node")
has_go          = enabled("runtimes.go")
has_rust        = enabled("runtimes.rust")
has_bun         = enabled("runtimes.bun")
has_claude_code = enabled("ai_tools.claude_code")
has_aider       = enabled("ai_tools.aider")
has_codex       = enabled("ai_tools.codex")
has_copilot     = enabled("ai_tools.copilot")
has_aiwg        = enabled("aiwg")
has_gpu         = enabled("resources.gpu")

any_runtime   = any([has_python, has_node, has_go, has_rust, has_bun])
any_user_tool = any([has_python, has_node, has_go, has_rust, has_bun,
                     has_claude_code, has_aider, has_codex, has_copilot, has_aiwg])

# ── helper: indent a multi-line string for YAML block scalar ──────────────────
def indent(text, spaces=6):
    pad = " " * spaces
    return "\n".join(pad + line if line.strip() else "" for line in text.splitlines())

# ── packages ───────────────────────────────────────────────────────────────────
def render_packages(pkgs):
    if not pkgs:
        return ""
    lines = ["packages:"]
    for p in pkgs:
        lines.append(f"  - {p}")
    return "\n".join(lines)

# ── health server (inline Python script) ──────────────────────────────────────
HEALTH_SERVER_PY = """#!/usr/bin/env python3
# Secured health check server for agentic-sandbox VMs - port 8118
# Security: Bearer token auth, rate limiting, no /logs/* endpoint
import http.server, json, os, subprocess, time, hmac
from datetime import datetime
PORT = 8118
BOOT_TIME = time.time()
AUTH_TOKEN_PATH = "/etc/agentic-sandbox/health-token"
LOG_DIR = "/var/log"
AGENT_STDOUT = f"{LOG_DIR}/agent-stdout.log"
AGENT_STDERR = f"{LOG_DIR}/agent-stderr.log"
RATE_LIMIT, RATE_WINDOW, REQUEST_COUNTS = 60, 60, {}

def load_auth_token():
    try:
        with open(AUTH_TOKEN_PATH) as f: return f.read().strip()
    except: return None
AUTH_TOKEN = load_auth_token()

def is_rate_limited(ip):
    now = time.time()
    if ip not in REQUEST_COUNTS:
        REQUEST_COUNTS[ip] = (1, now)
        return False
    count, window_start = REQUEST_COUNTS[ip]
    if now - window_start > RATE_WINDOW:
        REQUEST_COUNTS[ip] = (1, now)
        return False
    if count >= RATE_LIMIT: return True
    REQUEST_COUNTS[ip] = (count + 1, window_start)
    return False

class SecuredHealthHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args): pass
    def check_auth(self):
        if not AUTH_TOKEN: return True
        auth = self.headers.get("Authorization", "")
        if auth.startswith("Bearer "):
            return hmac.compare_digest(auth[7:].encode(), AUTH_TOKEN.encode())
        return False
    def send_json(self, data, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())
    def do_GET(self):
        if is_rate_limited(self.client_address[0]):
            self.send_json({"error": "rate_limit_exceeded"}, 429)
            return
        if self.path == "/ready":
            ready = os.path.exists("/var/run/agentic-setup-complete") or os.path.exists("/var/run/cloud-init-complete")
            self.send_json({"ready": ready}, 200 if ready else 503)
            return
        if self.path in ("/health", "/"):
            if not self.check_auth():
                self.send_json({"status": "healthy"})
                return
            self.send_json(self.collect_health())
            return
        if not self.check_auth():
            self.send_json({"error": "authentication_required"}, 401)
            return
        if self.path.startswith("/stream/"):
            stream_type = self.path[8:]
            if stream_type == "stdout": self.stream_file(AGENT_STDOUT)
            elif stream_type == "stderr": self.stream_file(AGENT_STDERR)
            else: self.send_json({"error": "not_found"}, 404)
            return
        self.send_json({"error": "not_found"}, 404)
    def stream_file(self, file_path):
        if not os.path.exists(file_path):
            self.send_json({"error": "file_not_found"}, 404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        try:
            with open(file_path, "r") as f:
                for line in f.read().split("\n"):
                    self.wfile.write(f"data: {line}\n\n".encode())
                self.wfile.flush()
            proc = subprocess.Popen(["tail", "-f", "-n", "0", file_path], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            try:
                for line in proc.stdout:
                    self.wfile.write(f"data: {line.decode().rstrip()}\n\n".encode())
                    self.wfile.flush()
            except: pass
            finally: proc.terminate()
        except Exception as e:
            self.wfile.write(f"data: Error: {e}\n\n".encode())
    def collect_health(self):
        return {"status": "healthy", "hostname": os.uname().nodename,
                "uptime_seconds": int(time.time() - BOOT_TIME),
                "timestamp": datetime.utcnow().isoformat() + "Z",
                "cloud_init_complete": os.path.exists("/var/run/cloud-init-complete"),
                "setup_complete": os.path.exists("/var/run/agentic-setup-complete"),
                "load_avg": list(os.getloadavg()),
                "streams": {"stdout": os.path.exists(AGENT_STDOUT), "stderr": os.path.exists(AGENT_STDERR)}}

if __name__ == "__main__":
    http.server.HTTPServer(("0.0.0.0", PORT), SecuredHealthHandler).serve_forever()
"""

# ── bashrc-additions (generated from active runtimes) ─────────────────────────
def build_bashrc_additions():
    lines = ["# === Agentic Development Environment ==="]
    lines.append("# Local bin")
    lines.append('export PATH="$HOME/.local/bin:$PATH"')

    if has_node and get("runtimes.node.method", "fnm") == "fnm":
        lines.append("# fnm")
        lines.append('export PATH="$HOME/.local/share/fnm:$PATH"')
        lines.append('eval "$(fnm env --use-on-cd 2>/dev/null)" || true')
        pkg_mgr = get("runtimes.node.package_manager", "pnpm")
        if pkg_mgr == "pnpm":
            lines.append("# pnpm")
            lines.append('export PNPM_HOME="$HOME/.local/share/pnpm"')
            lines.append('case ":$PATH:" in *":$PNPM_HOME:"*) ;; *) export PATH="$PNPM_HOME:$PATH" ;; esac')

    if has_bun:
        lines.append("# Bun")
        lines.append('export BUN_INSTALL="$HOME/.bun"')
        lines.append('export PATH="$BUN_INSTALL/bin:$PATH"')

    if has_go:
        lines.append("# Go")
        lines.append('export GOPATH="$HOME/.local/go"')
        lines.append('export PATH="/usr/local/go/bin:$GOPATH/bin:$PATH"')

    if has_rust:
        lines.append("# Rust")
        lines.append('source "$HOME/.cargo/env" 2>/dev/null || true')

    if has_python and get("runtimes.python.method", "uv") == "uv":
        lines.append("# uv")
        lines.append('export UV_CACHE_DIR="$HOME/.cache/uv"')

    if has_docker:
        lines.append("# Rootless Docker")
        lines.append('export XDG_RUNTIME_DIR="/run/user/$(id -u)"')
        lines.append('export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/docker.sock"')

    lines.append("# Disable telemetry")
    lines.append("export DISABLE_AUTOUPDATER=1")
    lines.append("export DISABLE_TELEMETRY=1")

    # Aliases for Ubuntu package naming quirks
    if "bat" in packages_list or "bat" in (get("packages") or []):
        lines.append("# Aliases")
        lines.append("alias bat='batcat'")
    if "fd-find" in packages_list:
        lines.append("alias fd='fdfind'")

    lines.append("# Prompt")
    lines.append(r"PS1='\[\e[36m\]\w\[\e[0m\] $ '")
    return "\n".join(lines)

# ── setup-user-tools.sh (generated from active features) ──────────────────────
def build_setup_user_tools():
    parts = []
    parts.append("""#!/bin/bash
# Do NOT use set -e — each tool install is independent
export HOME="/home/agent"
export PATH="$HOME/.local/bin:$PATH"
cd "$HOME"

TOOL_FAILURES=""
log() { echo "[user-tools] $1"; }
tool_fail() { log "ERROR: $1 failed"; TOOL_FAILURES="$TOOL_FAILURES $1"; }

# Retry wrapper with exponential backoff
retry() {
  local max=5 delay=3 attempt=1
  while [ $attempt -le $max ]; do
    if "$@"; then return 0; fi
    log "Attempt $attempt/$max failed, retrying in ${delay}s..."
    sleep $delay
    attempt=$((attempt + 1))
    delay=$((delay * 2))
  done
  log "ERROR: Failed after $max attempts"
  return 1
}""")

    if has_python and get("runtimes.python.method", "uv") == "uv":
        py_tools = get("runtimes.python.tools", [])
        py_tool_installs = "\n".join(f'retry uv tool install {tool} || tool_fail "uv-{tool}"' for tool in py_tools)
        parts.append(f"""
# uv - Python tooling
log "Installing uv..."
if retry sh -c 'curl -LsSf https://astral.sh/uv/install.sh | sh'; then
  export PATH="$HOME/.local/bin:$PATH"
  {py_tool_installs}
  log "uv installed"
else
  tool_fail "uv"
fi
""")

    if has_node and get("runtimes.node.method", "fnm") == "fnm":
        node_ver = get("runtimes.node.version", "lts")
        pkg_mgr  = get("runtimes.node.package_manager", "pnpm")
        global_pkgs = get("runtimes.node.global_packages", [])
        corepack_lines = ""
        if pkg_mgr == "pnpm":
            corepack_lines = "  corepack enable || true\n  corepack prepare pnpm@latest --activate || true"
        global_lines = ""
        if global_pkgs:
            global_lines = "  retry npm install -g " + " ".join(global_pkgs) + " || true"
        parts.append(f"""
# fnm - Fast Node Manager
log "Installing fnm..."
if retry sh -c 'curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell'; then
  export PATH="$HOME/.local/share/fnm:$PATH"
  # fnm env needs --shell flag in non-interactive contexts
  eval "$(fnm env --shell bash 2>/dev/null)" || true
  retry fnm install --{node_ver}
  fnm default {node_ver}-latest || true
  # Ensure node is on PATH for subsequent installs (fnm multishell dir)
  eval "$(fnm env --shell bash 2>/dev/null)" || true
  log "node=$(node --version 2>/dev/null || echo 'not found')"
{corepack_lines}
{global_lines}
  log "fnm + Node installed"
else
  tool_fail "fnm"
fi""")

    if has_bun:
        parts.append("""
# Bun
log "Installing Bun..."
if retry sh -c 'curl -fsSL https://bun.sh/install | bash'; then
  export PATH="$HOME/.bun/bin:$PATH"
  log "bun=$(bun --version 2>/dev/null || echo 'not found')"
  log "Bun installed"
else
  tool_fail "bun"
fi""")

    if has_rust:
        toolchain  = get("runtimes.rust.toolchain", "stable")
        profile    = get("runtimes.rust.profile", "minimal")
        components = get("runtimes.rust.components", [])
        crates     = get("runtimes.rust.crates", [])
        comp_line = ""
        if components:
            comp_line = "  rustup component add " + " ".join(components) + " || true"
        crate_line = ""
        if crates:
            crate_line = "  retry cargo install " + " ".join(crates) + " || tool_fail 'cargo-crates'"
        parts.append(f"""
# Rust
log "Installing Rust..."
if retry sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain {toolchain} --profile {profile}"; then
  source "$HOME/.cargo/env"
{comp_line}
{crate_line}
  log "Rust installed"
else
  tool_fail "rust"
fi
""")

    if has_go:
        go_tools = get("runtimes.go.tools", [])
        if go_tools:
            go_install_lines = "\n".join(f'  retry go install {tool} || tool_fail "go-tool-{tool.split("/")[-1].split("@")[0]}"' for tool in go_tools)
            parts.append(f"""
# Go tools
export GOPATH="$HOME/.local/go"
export PATH="/usr/local/go/bin:$GOPATH/bin:$PATH"
{go_install_lines}
""")

    if has_claude_code:
        cc_channel = get("ai_tools.claude_code.channel", "stable")
        cc_model   = get("ai_tools.claude_code.settings.model", "claude-sonnet-4-5-20250929")
        parts.append(f"""
# Claude Code CLI
log "Installing Claude Code ({cc_channel})..."
if retry sh -c 'curl -fsSL https://claude.ai/install.sh | bash -s {cc_channel}'; then
  export PATH="$HOME/.local/bin:$PATH"
  "$HOME/.local/bin/claude" install --yes 2>/dev/null || true
  mkdir -p "$HOME/.claude"
  echo '{{"model": "{cc_model}", "autoUpdatesChannel": "{cc_channel}"}}' > "$HOME/.claude/settings.json"
  log "Claude Code installed"
else
  tool_fail "claude-code"
fi
""")

    if has_aider:
        aider_model       = get("ai_tools.aider.config.model", "claude-3-5-sonnet-20241022")
        aider_edit_format = get("ai_tools.aider.config.edit_format", "diff")
        aider_auto_commits = "true" if get("ai_tools.aider.config.auto_commits", False) else "false"
        parts.append(f"""
# Aider config
log "Configuring Aider..."
cat > "$HOME/.aider.conf.yml" <<'AIDEREOF'
model: {aider_model}
edit-format: {aider_edit_format}
auto-commits: {aider_auto_commits}
attribute-commits: false
dark-mode: true
stream: true
check-update: false
analytics: false
AIDEREOF""")

    if has_codex:
        codex_model    = get("ai_tools.codex.config.model", "gpt-4o")
        codex_approval = get("ai_tools.codex.config.approval_mode", "suggest")
        parts.append(f"""
# Codex config
log "Configuring Codex..."
mkdir -p "$HOME/.codex"
cat > "$HOME/.codex/config.toml" <<'CODEXEOF'
[general]
model = "{codex_model}"
approval_mode = "{codex_approval}"
[output]
format = "json"
[git]
auto_commit = false
CODEXEOF""")

    if has_aiwg:
        frameworks = get("aiwg.frameworks", [])
        if frameworks:
            parts.append("""
# AIWG framework deployment
log "Deploying AIWG frameworks..."
export PATH="$HOME/.local/share/pnpm:$HOME/.local/share/fnm:$HOME/.bun/bin:$PATH"
eval "$(fnm env --shell bash 2>/dev/null)" || true
if command -v npm &>/dev/null; then
  npm install -g @jmagly/ai-writing-guide 2>/dev/null || log "WARN: aiwg npm install failed"
fi""")
            for fw in frameworks:
                fw_name = fw.get("name", "")
                for provider in fw.get("providers", []):
                    parts.append(f"if command -v aiwg &>/dev/null; then\n  retry aiwg use {fw_name} --provider {provider} || log 'WARN: aiwg use {fw_name} --provider {provider} failed'\nelse\n  log 'WARN: aiwg not available, skipping {fw_name} deployment'\nfi")

    parts.append("""
if [ -n "$TOOL_FAILURES" ]; then
  log "User tools setup complete with failures:$TOOL_FAILURES"
  exit 1
else
  log "User tools setup complete!"
fi
""")
    return "\n".join(parts)

# ── install.sh (root-level orchestrator) ──────────────────────────────────────
def build_install_sh():
    parts = []
    parts.append("""#!/bin/bash
# NOTE: Do NOT use set -e here. Each section handles its own errors so that
# a failure in one tool (e.g., rootless Docker) doesn't prevent the rest
# from installing.

TARGET_USER="agent"
USER_HOME="/home/$TARGET_USER"
LOG="/var/log/agentic-setup.log"
FAILURES=""

log() { echo "[$(date '+%H:%M:%S')] $1" | tee -a "$LOG"; }

record_failure() {
  log "ERROR: $1 failed"
  FAILURES="$FAILURES $1"
}

# ── Setup progress telemetry ──────────────────────────────────────────────────
# Writes JSON progress to /var/run/agentic-setup-progress.json so the agent
# can report setup state to the management server in heartbeats.
PROGRESS_FILE="/var/run/agentic-setup-progress.json"
STARTED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo '{"phase":"starting","started_at":"'"$STARTED_AT"'","steps":{}}' > "$PROGRESS_FILE"
chmod 644 "$PROGRESS_FILE"

report_progress() {
  local step="$1" status="$2"
  local now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  # Use Python for safe JSON update (jq may not be installed yet)
  python3 -c "
import json, sys
try:
    with open('$PROGRESS_FILE') as f: data = json.load(f)
except: data = {'phase':'unknown','steps':{}}
data['phase'] = '$status' if '$status' == 'complete' or '$status' == 'failed' else 'installing'
data['current_step'] = '$step'
data['updated_at'] = '$now'
data['steps']['$step'] = '$status'
with open('$PROGRESS_FILE','w') as f: json.dump(data, f)
" 2>/dev/null || true
}

# Retry wrapper with exponential backoff
retry() {
  local max_attempts=${RETRY_MAX:-5}
  local delay=${RETRY_DELAY:-5}
  local attempt=1
  while [ $attempt -le $max_attempts ]; do
    if "$@"; then return 0; fi
    log "Attempt $attempt/$max_attempts failed, retrying in ${delay}s..."
    sleep $delay
    attempt=$((attempt + 1))
    delay=$((delay * 2))
  done
  log "ERROR: Command failed after $max_attempts attempts: $*"
  return 1
}

log "Starting agentic-sandbox dev environment setup..."

# ── 1. Tool symlinks (Ubuntu package naming quirks) ──────────────────────────
log "Creating tool symlinks..."
mkdir -p "$USER_HOME/.local/bin"
ln -sf /usr/bin/batcat "$USER_HOME/.local/bin/bat" 2>/dev/null || true
ln -sf /usr/bin/fdfind "$USER_HOME/.local/bin/fd"  2>/dev/null || true
chown -R "$TARGET_USER:$TARGET_USER" "$USER_HOME/.local"
""")

    if has_docker:
        parts.append("""# ── 2. Rootless Docker (no docker group membership) ────────────────────────────
report_progress "docker" "installing"
log "Installing Rootless Docker..."

if (
# Subordinate UID/GID ranges for user namespaces
if ! grep -q "^$TARGET_USER:" /etc/subuid; then
    echo "$TARGET_USER:100000:65536" >> /etc/subuid
fi
if ! grep -q "^$TARGET_USER:" /etc/subgid; then
    echo "$TARGET_USER:100000:65536" >> /etc/subgid
fi

# Install Docker CE
install -m 0755 -d /etc/apt/keyrings
retry curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
chmod a+r /etc/apt/keyrings/docker.asc
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \\
  https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \\
  tee /etc/apt/sources.list.d/docker.list > /dev/null
retry apt-get update -q
retry sh -c 'DEBIAN_FRONTEND=noninteractive apt-get install -y \\
  docker-ce docker-ce-cli containerd.io \\
  docker-buildx-plugin docker-compose-plugin'

# DO NOT add user to docker group (security: eliminates privilege escalation)

# Stop system Docker daemon (not needed for rootless)
systemctl stop docker   || true
systemctl disable docker || true

# Enable lingering so user services survive without an active login session
loginctl enable-linger "$TARGET_USER"

USER_ID=$(id -u "$TARGET_USER")
mkdir -p "/run/user/$USER_ID"
chown "$TARGET_USER:$TARGET_USER" "/run/user/$USER_ID"
chmod 700 "/run/user/$USER_ID"

# Run rootless Docker setup as the agent user
sudo -u "$TARGET_USER" XDG_RUNTIME_DIR="/run/user/$USER_ID" /opt/agentic-setup/setup-rootless-docker.sh

# Allow unprivileged port binding (ports 80/443)
echo "net.ipv4.ip_unprivileged_port_start=80" > /etc/sysctl.d/99-rootless-docker.conf
sysctl -p /etc/sysctl.d/99-rootless-docker.conf

log "Rootless Docker installed"
); then
  log "Rootless Docker setup complete"
  report_progress "docker" "done"
else
  record_failure "rootless-docker"
  report_progress "docker" "failed"
fi
""")

    if has_go:
        go_version = get("runtimes.go.version", "latest")
        # Use a pinned version for "latest" since we can't resolve at write time
        go_ver_str = "1.24.3" if go_version == "latest" else go_version
        parts.append(f"""# ── 3. Go runtime (system-level install to /usr/local/go) ────────────────────
report_progress "go" "installing"
log "Installing Go {go_ver_str}..."
if (
install_go() {{
  wget -qO /tmp/go.tar.gz "https://go.dev/dl/go{go_ver_str}.linux-amd64.tar.gz" && \\
  rm -rf /usr/local/go && \\
  tar -C /usr/local -xzf /tmp/go.tar.gz && \\
  rm -f /tmp/go.tar.gz
}}
retry install_go
); then
  log "Go {go_ver_str} installed at /usr/local/go"
  report_progress "go" "done"
else
  record_failure "go"
  report_progress "go" "failed"
fi
""")

    if any_user_tool:
        parts.append("""# ── 4. User-level tools (runs as agent user) ──────────────────────────────────
report_progress "user-tools" "installing"
log "Installing user-level development tools..."
if sudo -u "$TARGET_USER" /opt/agentic-setup/setup-user-tools.sh; then
  log "User tools complete"
  report_progress "user-tools" "done"
else
  record_failure "user-tools"
  report_progress "user-tools" "failed"
fi
""")

    parts.append("""# ── 5. Git configuration ──────────────────────────────────────────────────────
log "Configuring git..."
sudo -u "$TARGET_USER" git config --global user.name "Sandbox Agent"
sudo -u "$TARGET_USER" git config --global user.email "agent@sandbox.local"
sudo -u "$TARGET_USER" git config --global init.defaultBranch main
sudo -u "$TARGET_USER" git config --global core.pager delta
sudo -u "$TARGET_USER" git config --global interactive.diffFilter 'delta --color-only'
sudo -u "$TARGET_USER" git config --global delta.navigate true
sudo -u "$TARGET_USER" git config --global delta.side-by-side true
""")

    if any_runtime:
        parts.append("""# ── 6. Shell integrations ─────────────────────────────────────────────────────
log "Configuring shell environment..."

# Write shell integrations to .bashrc (for interactive shells)
cat /opt/agentic-setup/bashrc-additions.sh >> "$USER_HOME/.bashrc"
chown "$TARGET_USER:$TARGET_USER" "$USER_HOME/.bashrc"

# Write environment file for all shell contexts (SSH commands, systemd, etc.)
# This goes in /etc/environment.d/ so it's picked up by PAM for SSH sessions
cat > /etc/environment.d/99-agentic-tools.conf <<'ENVEOF'
PATH=/home/agent/.local/bin:/home/agent/.cargo/bin:/home/agent/.local/share/fnm:/home/agent/.bun/bin:/usr/local/go/bin:/home/agent/.local/go/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
GOPATH=/home/agent/.local/go
BUN_INSTALL=/home/agent/.bun
ENVEOF

# Also prepend PATH exports to .profile BEFORE the .bashrc source line
# so they're available in login shells (ssh user@host 'bash -l -c ...')
PROFILE_INJECT='
# Agentic-sandbox tool paths
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$HOME/.local/share/fnm:$HOME/.bun/bin:/usr/local/go/bin:$HOME/.local/go/bin:$PATH"
export GOPATH="$HOME/.local/go"
export BUN_INSTALL="$HOME/.bun"
eval "$(fnm env --shell bash 2>/dev/null)" || true
'
# Insert after the first line of .profile
sed -i "1a\\${PROFILE_INJECT}" "$USER_HOME/.profile" 2>/dev/null || \
  echo "$PROFILE_INJECT" >> "$USER_HOME/.profile"
chown "$TARGET_USER:$TARGET_USER" "$USER_HOME/.profile"
""")

    if has_go:
        parts.append("""# Go paths in .profile (login shells don't always source .bashrc)
printf '\\n# Go\\nexport GOPATH="$HOME/.local/go"\\nexport PATH="/usr/local/go/bin:$GOPATH/bin:$PATH"\\n' \\
    >> "$USER_HOME/.profile"
chown "$TARGET_USER:$TARGET_USER" "$USER_HOME/.profile"
""")

    parts.append("""# ── 7. Generate ENVIRONMENT.md ────────────────────────────────────────────────
report_progress "env-docs" "installing"
log "Generating ENVIRONMENT.md..."
/opt/agentic-setup/generate-env-docs.sh || true
report_progress "env-docs" "done"

# ── 8. Mark setup complete ────────────────────────────────────────────────────
touch /var/run/agentic-setup-complete

if [ -n "$FAILURES" ]; then
  log "Agentic-sandbox setup complete with failures:$FAILURES"
  report_progress "complete-with-errors" "complete"
else
  log "Agentic-sandbox setup complete!"
  report_progress "complete" "complete"
fi

# Final checkin with host
CHECKIN_HOST="$(ip route | grep default | awk '{print $3}')"
MY_IP="$(hostname -I | awk '{print $1}')"
curl -sf -X POST "http://${CHECKIN_HOST}:8119/checkin" \\
  -H "Content-Type: application/json" \\
  -d "{\\"name\\": \\"$(hostname)\\", \\"ip\\": \\"${MY_IP}\\", \\"status\\": \\"ready\\", \\"message\\": \\"Agentic dev environment ready\\"}" \\
  2>/dev/null || log "Checkin server not available (OK)"
""")

    return "\n".join(parts)

# ── generate-env-docs.sh ───────────────────────────────────────────────────────
def build_env_docs_sh():
    installed_tools = []
    if has_python: installed_tools.append("uv (Python)")
    if has_node:   installed_tools.append("fnm/node")
    if has_go:     installed_tools.append("Go")
    if has_rust:   installed_tools.append("Rust")
    if has_bun:    installed_tools.append("Bun")
    if has_docker: installed_tools.append("Rootless Docker")
    if has_claude_code: installed_tools.append("Claude Code")
    if has_aider:  installed_tools.append("Aider")
    if has_codex:  installed_tools.append("Codex")

    tools_summary = ", ".join(installed_tools) if installed_tools else "base tools"

    return f"""#!/bin/bash
# generate-env-docs.sh - Generate ENVIRONMENT.md based on installed tools
export HOME="/home/agent"
export GOPATH="$HOME/.local/go"
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$HOME/.local/share/fnm:$HOME/.bun/bin:/usr/local/go/bin:$GOPATH/bin:$PATH"
eval "$($HOME/.local/share/fnm/fnm env 2>/dev/null)" || true

OUTPUT="$HOME/ENVIRONMENT.md"

get_version() {{
  local result
  result=$($1 ${{2:---version}} 2>/dev/null | head -1) || true
  echo "${{result:-not installed}}"
}}

cat > "$OUTPUT" <<'ENVMD'
# Agentic Development Environment

**Generated:** $(date -Iseconds)
**Tools:** {tools_summary}

## Usage Patterns

| Task | Command |
|------|---------|
| Search code | `rg pattern` |
| Find files | `fd pattern` |
| HTTP requests | `curl` or `xh` |
| JSON processing | `jq` |
ENVMD

# Append runtime versions
echo "" >> "$OUTPUT"
echo "## Installed Versions" >> "$OUTPUT"
echo "" >> "$OUTPUT"
echo "| Tool | Version |" >> "$OUTPUT"
echo "|------|---------|" >> "$OUTPUT"
{"echo \"| uv | $(get_version uv --version) |\" >> \"$OUTPUT\"" if has_python else ""}
{"echo \"| node | $(get_version node --version) |\" >> \"$OUTPUT\"" if has_node else ""}
{"echo \"| go | $(get_version go version) |\" >> \"$OUTPUT\"" if has_go else ""}
{"echo \"| rustc | $(get_version rustc --version) |\" >> \"$OUTPUT\"" if has_rust else ""}
{"echo \"| bun | $(get_version bun --version) |\" >> \"$OUTPUT\"" if has_bun else ""}
{"echo \"| docker | $(get_version docker --version) |\" >> \"$OUTPUT\"" if has_docker else ""}
{"echo \"| claude | $(get_version claude --version) |\" >> \"$OUTPUT\"" if has_claude_code else ""}
{"echo \"| aider | $(get_version aider --version) |\" >> \"$OUTPUT\"" if has_aider else ""}

chown agent:agent "$OUTPUT"
echo "Generated $OUTPUT"
"""

# ── setup-rootless-docker.sh ──────────────────────────────────────────────────
ROOTLESS_DOCKER_SH = """#!/bin/bash
export HOME="/home/agent"
export PATH="$HOME/.local/bin:/usr/bin:$PATH"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"

# Install rootless Docker
dockerd-rootless-setuptool.sh install || {
  echo "WARNING: dockerd-rootless-setuptool.sh failed, Docker may need manual setup"
  exit 1
}

mkdir -p "$HOME/.docker"
echo '{"currentContext": "rootless"}' > "$HOME/.docker/config.json"

# systemctl --user may not work during cloud-init (no user session bus).
# Try to enable, but don't fail if it can't - lingering + first login will fix it.
systemctl --user enable docker 2>/dev/null || true
systemctl --user start docker 2>/dev/null || true
"""

# ── Claude managed-settings ───────────────────────────────────────────────────
def build_claude_managed_settings():
    ms = get("ai_tools.claude_code.managed_settings", {})
    if not ms:
        ms = {
            "permissions": {
                "deny": ["Bash(rm -rf /*)"],
                "allow": ["Read", "Edit", "Write", "Glob", "Grep",
                          "Bash(git *)", "Bash(cargo *)", "Bash(npm *)", "Bash(pnpm *)"]
            }
        }
    return json.dumps(ms, indent=2)

# ── assemble write_files ───────────────────────────────────────────────────────
def yaml_block(content, indent_spaces=6):
    """Render content as a YAML literal block scalar, indented."""
    pad = " " * indent_spaces
    lines = content.rstrip("\n").split("\n")
    return "\n".join(pad + line for line in lines)

# Build the write_files entries as a list of (path, permissions, owner, content) tuples
write_files_entries = []

# Always present
write_files_entries.append({
    "path": "/opt/agentic-sandbox/health/health-server.py",
    "permissions": "0755",
    "content": HEALTH_SERVER_PY,
})
write_files_entries.append({
    "path": "/etc/agentic-sandbox/health-token",
    "permissions": "0600",
    "owner": "root:root",
    "content": "HEALTH_TOKEN_PLACEHOLDER\n",
})
write_files_entries.append({
    "path": "/etc/systemd/system/agentic-health.service",
    "content": """\
[Unit]
Description=Agentic Sandbox Health Check Server
After=network.target
[Service]
Type=simple
ExecStart=/usr/bin/python3 /opt/agentic-sandbox/health/health-server.py
Restart=always
RestartSec=5
[Install]
WantedBy=multi-user.target
""",
})
write_files_entries.append({
    "path": "/etc/systemd/system/agentic-agent.service",
    "content": """\
[Unit]
Description=Agentic Sandbox Agent Client
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
User=agent
EnvironmentFile=/etc/agentic-sandbox/agent.env
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/agentic-agent --server MANAGEMENT_SERVER_PLACEHOLDER --agent-id VM_NAME_PLACEHOLDER --secret AGENT_SECRET_PLACEHOLDER
Restart=always
RestartSec=5
[Install]
WantedBy=multi-user.target
""",
})
write_files_entries.append({
    "path": "/etc/agentic-sandbox/agent.env",
    "permissions": "0600",
    "owner": "root:root",
    "content": f"""\
# Agent identification and authentication
AGENT_ID=VM_NAME_PLACEHOLDER
AGENT_SECRET=AGENT_SECRET_PLACEHOLDER
MANAGEMENT_SERVER=MANAGEMENT_SERVER_PLACEHOLDER
AGENT_LOADOUT={get("metadata.name", "")}
# Set at provisioning time - do not modify
""",
})
write_files_entries.append({
    "path": "/opt/agentic-setup/check-ready.sh",
    "permissions": "0755",
    "content": """\
#!/bin/bash
[ -f /var/run/agentic-setup-complete ] && echo "ready" && exit 0
echo "pending" && exit 1
""",
})
write_files_entries.append({
    "path": "/opt/agentic-setup/install.sh",
    "permissions": "0755",
    "content": build_install_sh(),
})
write_files_entries.append({
    "path": "/opt/agentic-sandbox/generate-env-docs.sh",
    "permissions": "0755",
    "content": build_env_docs_sh(),
})
write_files_entries.append({
    "path": "/etc/profile.d/99-agentic-welcome.sh",
    "permissions": "0644",
    "content": r"""#!/bin/bash
[[ $- != *i* ]] && return
[[ "$PWD" == "/opt/agentic-sandbox" || "$PWD" == "/" ]] && cd "$HOME" 2>/dev/null

if [ -t 1 ]; then
    C="\e[36m"; B="\e[1m"; Y="\e[33m"; G="\e[32m"; R="\e[0m"
    H=$(hostname)
    TITLE=" Agentic Sandbox - $H"
    PAD=$((55 - ${#TITLE}))
    TITLE_PAD="${TITLE}$(printf "%${PAD}s" "")"

    echo ""
    echo -e "${C}╭───────────────────────────────────────────────────────╮${R}"
    echo -e "${C}│${R}${B}${TITLE_PAD}${R}${C}│${R}"
    echo -e "${C}├───────────────────────────────────────────────────────┤${R}"
    echo -e "${C}│${R}                                                       ${C}│${R}"
    echo -e "${C}│${R} ${Y}Quick Reference:${R}                                      ${C}│${R}"
    echo -e "${C}│${R}   rg PATTERN           Search code                    ${C}│${R}"
    echo -e "${C}│${R}   fd PATTERN           Find files                     ${C}│${R}"
    echo -e "${C}│${R}                                                       ${C}│${R}"
    echo -e "${C}│${R} ${G}Docs:${R}  ~/ENVIRONMENT.md                               ${C}│${R}"
    echo -e "${C}│${R}                                                       ${C}│${R}"
    echo -e "${C}╰───────────────────────────────────────────────────────╯${R}"
    echo ""
fi
""",
})

# Conditional write_files
if has_docker:
    write_files_entries.append({
        "path": "/opt/agentic-setup/setup-rootless-docker.sh",
        "permissions": "0755",
        "content": ROOTLESS_DOCKER_SH,
    })

if any_runtime:
    write_files_entries.append({
        "path": "/opt/agentic-setup/bashrc-additions.sh",
        "permissions": "0644",
        "content": build_bashrc_additions() + "\n",
    })

if any_user_tool:
    write_files_entries.append({
        "path": "/opt/agentic-setup/setup-user-tools.sh",
        "permissions": "0755",
        "content": build_setup_user_tools(),
    })

if has_claude_code:
    write_files_entries.append({
        "path": "/etc/claude-code/managed-settings.json",
        "permissions": "0644",
        "content": build_claude_managed_settings() + "\n",
    })

# Manifest custom write_files (appended last)
for wf in get("write_files", []):
    write_files_entries.append(wf)

# ── assemble runcmd ────────────────────────────────────────────────────────────
runcmd_entries = []

# 1. host.internal
runcmd_entries.append("- echo \"MANAGEMENT_HOST_IP_PLACEHOLDER host.internal\" >> /etc/hosts")

# 2. Timezone
runcmd_entries.append("- timedatectl set-timezone UTC")

# 3. Secrets directory
runcmd_entries.append("- mkdir -p /etc/agentic-sandbox")
runcmd_entries.append("- chmod 700 /etc/agentic-sandbox")

# 4. UFW
ufw_block = """\
- |
  NETWORK_MODE="NETWORK_MODE_PLACEHOLDER"
  MGMT_IP="MANAGEMENT_HOST_IP_PLACEHOLDER"
  echo "Configuring UFW (network mode: $NETWORK_MODE)"
  ufw default deny incoming
  ufw allow from $MGMT_IP to any port 22   proto tcp comment 'SSH from management host'
  ufw allow from $MGMT_IP to any port 8118 proto tcp comment 'Health check from management host'
  case "$NETWORK_MODE" in
    isolated)
      ufw default deny outgoing
      ufw allow out to $MGMT_IP port 8120 proto tcp comment 'gRPC to management'
      ufw allow out to $MGMT_IP port 8121 proto tcp comment 'WebSocket to management'
      ufw allow out to $MGMT_IP port 8122 proto tcp comment 'HTTP to management'
      ufw allow out on lo
      echo "[UFW] isolated mode - management server only"
      ;;
    allowlist)
      ufw default deny outgoing
      ufw allow out to $MGMT_IP port 8120 proto tcp comment 'gRPC'
      ufw allow out to $MGMT_IP port 8121 proto tcp comment 'WebSocket'
      ufw allow out to $MGMT_IP port 8122 proto tcp comment 'HTTP'
      ufw allow out to $MGMT_IP port 53   comment 'DNS to filtered resolver'
      ufw allow out to any port 443 proto tcp comment 'HTTPS (DNS-filtered)'
      ufw allow out to any port 80  proto tcp comment 'HTTP (DNS-filtered)'
      ufw deny  out to 8.8.8.8 port 53 comment 'Block external DNS'
      ufw deny  out to 8.8.4.4 port 53
      ufw deny  out to 1.1.1.1 port 53
      ufw deny  out to any     port 853 comment 'Block DoT'
      ufw allow out on lo
      echo "[UFW] allowlist mode - DNS filtered + HTTPS"
      ;;
    full|*)
      ufw default allow outgoing
      echo "[UFW] full mode - unrestricted egress"
      ;;
  esac
  echo "y" | ufw enable
  ufw status verbose >> /var/log/ufw-setup.log"""
runcmd_entries.append(ufw_block)

# 5. qemu-guest-agent
runcmd_entries.append("- systemctl enable qemu-guest-agent")
runcmd_entries.append("- systemctl start qemu-guest-agent")

# 6. health server
runcmd_entries.append("- systemctl daemon-reload")
runcmd_entries.append("- systemctl enable agentic-health")
runcmd_entries.append("- systemctl start agentic-health")

# 7. Agentshare virtiofs mounts (must happen BEFORE agent binary install)
if use_agentshare:
    runcmd_entries.append("""\
- mkdir -p /mnt/global /mnt/inbox /mnt/outbox
- |
  echo "# Agentshare virtiofs mounts" >> /etc/fstab
  echo "agentglobal  /mnt/global  virtiofs ro,noatime,nofail 0 0" >> /etc/fstab
  echo "agentinbox   /mnt/inbox   virtiofs rw,noatime,nofail 0 0" >> /etc/fstab
  echo "agentoutbox  /mnt/outbox  virtiofs rw,noatime,nofail 0 0" >> /etc/fstab
- mount -t virtiofs agentglobal /mnt/global   || echo "agentglobal mount not available yet"
- mount -t virtiofs agentinbox  /mnt/inbox    || echo "agentinbox mount not available yet"
- mount -t virtiofs agentoutbox /mnt/outbox   || echo "agentoutbox mount not available yet"
- ln -sfn /mnt/global /home/agent/global
- ln -sfn /mnt/inbox  /home/agent/inbox
- ln -sfn /mnt/inbox  /home/agent/workspace
- ln -sfn /mnt/outbox /home/agent/outbox
- chown -h agent:agent /home/agent/global /home/agent/inbox /home/agent/workspace /home/agent/outbox
- |
  mkdir -p /mnt/outbox/progress /mnt/outbox/artifacts
  chown -R agent:agent /mnt/outbox/progress /mnt/outbox/artifacts
- |
  RUN_ID="run-$(date +%Y%m%d-%H%M%S)"
  mkdir -p /mnt/inbox/runs/$RUN_ID/{outputs,trace}
  ln -sfn /mnt/inbox/runs/$RUN_ID /mnt/inbox/current
  chown -R agent:agent /mnt/inbox/runs/$RUN_ID""")

# 8. Agent binary install (from virtiofs global share, now mounted above)
runcmd_entries.append("""\
- |
  for i in $(seq 1 30); do
    if [ -f /mnt/global/bin/agentic-agent ]; then
      cp /mnt/global/bin/agentic-agent /usr/local/bin/agentic-agent
      chmod 755 /usr/local/bin/agentic-agent
      systemctl daemon-reload
      systemctl enable agentic-agent
      systemctl start agentic-agent
      echo "Agent installed from global share (attempt $i)"
      break
    fi
    echo "Waiting for agentic-agent in global share (attempt $i/30)..."
    sleep 2
  done
  if [ ! -f /usr/local/bin/agentic-agent ]; then
    echo "Agent binary not found after 60s - will need manual deployment"
    echo "Run: ./scripts/deploy-agent.sh VM_NAME_PLACEHOLDER"
    systemctl enable agentic-agent || true
  fi""")

# 9. Setup directories for user tools
runcmd_entries.append("- mkdir -p /home/agent/.local/bin")
runcmd_entries.append("- chown -R agent:agent /home/agent/.local")

# 10. Cloud-init complete marker
runcmd_entries.append("- touch /var/run/cloud-init-complete")

# 11. Initial checkin
runcmd_entries.append("""\
- |
  CHECKIN_HOST="$(ip route | grep default | awk '{print $3}')"
  MY_IP="$(hostname -I | awk '{print $1}')"
  curl -sf -X POST "http://${CHECKIN_HOST}:8119/checkin" \\
    -H "Content-Type: application/json" \\
    -d "{\\"name\\": \\"$(hostname)\\", \\"ip\\": \\"${MY_IP}\\", \\"status\\": \\"setup\\", \\"message\\": \\"Cloud-init complete, setup starting\\"}" \\
    2>/dev/null || true""")

# 12. Run install.sh in background
runcmd_entries.append("- nohup /opt/agentic-setup/install.sh > /var/log/agentic-setup.log 2>&1 &")

# GPU driver setup (if GPU passthrough enabled)
if has_gpu:
    runcmd_entries.append("- apt-get install -y --no-install-recommends ubuntu-drivers-common")
    runcmd_entries.append("- ubuntu-drivers install --gpgpu || true")

# Append any custom runcmd from the manifest
for cmd in get("runcmd", []):
    runcmd_entries.append(f"- {cmd}")

# ── render write_files YAML ────────────────────────────────────────────────────
def render_write_files(entries):
    out = ["write_files:"]
    for e in entries:
        path        = e.get("path", "")
        permissions = e.get("permissions", "")
        owner       = e.get("owner", "")
        content     = e.get("content", "")

        out.append(f"  - path: {path}")
        if permissions:
            out.append(f"    permissions: '{permissions}'")
        if owner:
            out.append(f"    owner: {owner}")
        out.append("    content: |")
        for line in content.rstrip("\n").split("\n"):
            out.append("      " + line)
        out.append("")
    return "\n".join(out)

# ── render packages YAML ───────────────────────────────────────────────────────
pkgs_yaml = render_packages(packages_list)

# ── render runcmd YAML ─────────────────────────────────────────────────────────
def render_runcmd(entries):
    out = ["runcmd:"]
    for e in entries:
        # Entries that start with "- " are already formatted
        if e.startswith("- "):
            # indent continuation lines
            lines = e.split("\n")
            out.append("  " + lines[0])
            for l in lines[1:]:
                out.append("  " + l)
        else:
            out.append("  " + e)
    return "\n".join(out)

# ── assemble final user-data ───────────────────────────────────────────────────
parts = []
parts.append("#cloud-config")
parts.append("")
parts.append("hostname: VM_NAME_PLACEHOLDER")
parts.append("manage_etc_hosts: true")
parts.append("")
parts.append("# Users")
parts.append("# - agent: primary service account (debug key + ephemeral automation key)")
parts.append("# - root:  emergency access only")
parts.append("users:")
parts.append("  - name: agent")
parts.append("    groups: [sudo]")
parts.append("    shell: /bin/bash")
parts.append("    sudo: ALL=(ALL) NOPASSWD:ALL")
parts.append("    ssh_authorized_keys:")
parts.append("      - SSH_KEY_PLACEHOLDER")
parts.append("      - EPHEMERAL_SSH_KEY_PLACEHOLDER")
parts.append("  - name: root")
parts.append("    ssh_authorized_keys:")
parts.append("      - SSH_KEY_PLACEHOLDER")
parts.append("")
parts.append("package_update: true")
parts.append("")
if pkgs_yaml:
    parts.append(pkgs_yaml)
    parts.append("")
parts.append(render_write_files(write_files_entries))
parts.append("")
parts.append(render_runcmd(runcmd_entries))
parts.append("")
parts.append('final_message: "VM VM_NAME_PLACEHOLDER provisioned. Setup running in background - check /var/log/agentic-setup.log and ~/ENVIRONMENT.md"')

output = "\n".join(parts)

with open(output_path, "w") as f:
    f.write(output)

print(f"Generated: {output_path}")

# Write GPU config sidecar if GPU passthrough is enabled
if has_gpu:
    gpu_device = get("resources.gpu.device", "")
    gpu_driver = get("resources.gpu.driver", "vfio-pci")
    gpu_config_path = output_path.replace("user-data", "gpu-config")
    with open(gpu_config_path, "w") as gf:
        gf.write(f"GPU_ENABLED=true\n")
        gf.write(f"GPU_PCI_DEVICE={gpu_device}\n")
        gf.write(f"GPU_DRIVER={gpu_driver}\n")
    print(f"GPU config: {gpu_config_path}")
PYTHON_EOF

# ── sed substitutions for placeholders ────────────────────────────────────────
# Replace EPHEMERAL_ first to avoid partial-match with SSH_KEY_PLACEHOLDER
sed -i "s|EPHEMERAL_SSH_KEY_PLACEHOLDER|${EPHEMERAL_PUBKEY}|g"   "$OUTPUT_DIR/user-data"
sed -i "s|SSH_KEY_PLACEHOLDER|${SSH_PUBKEY}|g"                    "$OUTPUT_DIR/user-data"
sed -i "s|VM_NAME_PLACEHOLDER|${VM_NAME}|g"                       "$OUTPUT_DIR/user-data"
sed -i "s|AGENT_SECRET_PLACEHOLDER|${AGENT_SECRET}|g"             "$OUTPUT_DIR/user-data"
sed -i "s|HEALTH_TOKEN_PLACEHOLDER|${HEALTH_TOKEN}|g"             "$OUTPUT_DIR/user-data"
sed -i "s|MANAGEMENT_SERVER_PLACEHOLDER|${MANAGEMENT_SERVER}|g"   "$OUTPUT_DIR/user-data"
sed -i "s|NETWORK_MODE_PLACEHOLDER|${NETWORK_MODE_ARG:-full}|g"   "$OUTPUT_DIR/user-data"

# Derive the management host IP from the management server address for UFW rules.
# If MANAGEMENT_SERVER is a hostname:port pair, attempt to resolve it;
# otherwise fall back to the standard libvirt default gateway.
MGMT_HOST="${MANAGEMENT_SERVER%%:*}"
MGMT_IP=""
if [[ "$MGMT_HOST" == "host.internal" ]]; then
    MGMT_IP="192.168.122.1"
elif [[ "$MGMT_HOST" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    MGMT_IP="$MGMT_HOST"
else
    MGMT_IP=$(getent hosts "$MGMT_HOST" 2>/dev/null | awk '{print $1; exit}') || true
    MGMT_IP="${MGMT_IP:-192.168.122.1}"
fi

sed -i "s|MANAGEMENT_HOST_IP_PLACEHOLDER|${MGMT_IP}|g" "$OUTPUT_DIR/user-data"

# ── meta-data (required for cloud-init ISO) ──────────────────────────────────
cat > "$OUTPUT_DIR/meta-data" <<EOF
instance-id: ${VM_NAME}-$(date +%s)
local-hostname: ${VM_NAME}
EOF

echo "user-data written to ${OUTPUT_DIR}/user-data"

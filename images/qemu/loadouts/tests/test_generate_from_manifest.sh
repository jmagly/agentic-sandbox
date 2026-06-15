#!/bin/bash
# tests/test_generate_from_manifest.sh
#
# Smoke-tests for generate-from-manifest.sh.
# Requires: bash, python3+PyYAML, the loadout layer/profile YAML files.
#
# Usage:  ./tests/test_generate_from_manifest.sh
# Exit:   0 if all tests pass, non-zero otherwise.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOADOUTS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
GENERATE="$LOADOUTS_DIR/generate-from-manifest.sh"
RESOLVE="$LOADOUTS_DIR/resolve-manifest.sh"

# ── test harness ───────────────────────────────────────────────────────────────
PASS=0
FAIL=0
ERRORS=()

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1"; FAIL=$((FAIL + 1)); ERRORS+=("$1"); }

assert_contains() {
    local label="$1" needle="$2" file="$3"
    if grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected to find: $needle)"
    fi
}

assert_not_contains() {
    local label="$1" needle="$2" file="$3"
    if ! grep -qF -- "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected NOT to find: $needle)"
    fi
}

assert_exits_ok() {
    local label="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        pass "$label"
    else
        fail "$label (command exited non-zero)"
    fi
}

assert_yaml_ok() {
    local label="$1" file="$2"
    if python3 - "$file" <<'PY' >/dev/null 2>&1
import sys
import yaml
with open(sys.argv[1]) as f:
    yaml.safe_load(f)
PY
    then
        pass "$label"
    else
        fail "$label (YAML parse failed)"
    fi
}

# ── shared test fixtures ───────────────────────────────────────────────────────
TMPDIR_ROOT=$(mktemp -d /tmp/test-generate.XXXXXX)
trap 'rm -rf "$TMPDIR_ROOT"' EXIT

VM_NAME="test-vm-01"
SSH_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKEYEXAMPLE user@host"
EPHEMERAL_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEPHEMERALEXAMPLE ephemeral"
AGENT_SECRET="deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
HEALTH_TOKEN="cafecafecafecafecafecafecafecafecafecafecafecafecafecafecafecafe"
MAC_ADDRESS="52:54:00:ab:cd:ef"
MGMT_SERVER="host.internal:8120"

run_generate() {
    local manifest="$1"
    local outdir="$2"
    local network_mode="${3:-full}"
    local agentshare="${4:-false}"
    mkdir -p "$outdir"
    AGENT_BOOTSTRAP_TOKEN="bootstrap-token-not-real" \
    AGENT_BOOTSTRAP_SPIFFE_ID="spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" \
    AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS="1900000000000" \
        "$GENERATE" "$manifest" "$VM_NAME" "$SSH_KEY" "$outdir" \
            "$agentshare" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" \
            "$network_mode" "$HEALTH_TOKEN" "$MGMT_SERVER"
}

run_generate_insecure() {
    local manifest="$1"
    local outdir="$2"
    local network_mode="${3:-full}"
    local agentshare="${4:-false}"
    mkdir -p "$outdir"
    "$GENERATE" "$manifest" "$VM_NAME" "$SSH_KEY" "$outdir" \
        "$agentshare" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" \
        "$network_mode" "$HEALTH_TOKEN" "$MGMT_SERVER"
}

run_generate_tls() {
    local manifest="$1"
    local outdir="$2"
    local network_mode="${3:-full}"
    local agentshare="${4:-false}"
    local tls_dir="$TMPDIR_ROOT/tls"
    mkdir -p "$outdir" "$tls_dir"
    printf '%s\n' 'test-ca' > "$tls_dir/ca.pem"
    printf '%s\n' 'test-cert' > "$tls_dir/agent.pem"
    printf '%s\n' 'test-key' > "$tls_dir/agent-key.pem"
    AGENT_GRPC_TLS_CA_HOST_PATH="$tls_dir/ca.pem" \
    AGENT_GRPC_TLS_CERT_HOST_PATH="$tls_dir/agent.pem" \
    AGENT_GRPC_TLS_KEY_HOST_PATH="$tls_dir/agent-key.pem" \
    AGENT_GRPC_TLS_SERVER_NAME="host.internal" \
    AGENT_BOOTSTRAP_TOKEN="bootstrap-token-not-real" \
    AGENT_BOOTSTRAP_SPIFFE_ID="spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" \
    AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS="1900000000000" \
        "$GENERATE" "$manifest" "$VM_NAME" "$SSH_KEY" "$outdir" \
            "$agentshare" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" \
            "$network_mode" "$HEALTH_TOKEN" "$MGMT_SERVER"
}

run_generate_bootstrap() {
    local manifest="$1"
    local outdir="$2"
    local network_mode="${3:-full}"
    local agentshare="${4:-false}"
    mkdir -p "$outdir"
    AGENT_BOOTSTRAP_TOKEN="bootstrap-token-not-real" \
    AGENT_BOOTSTRAP_SPIFFE_ID="spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" \
    AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS="1900000000000" \
        "$GENERATE" "$manifest" "$VM_NAME" "$SSH_KEY" "$outdir" \
            "$agentshare" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" \
            "$network_mode" "$HEALTH_TOKEN" "$MGMT_SERVER"
}

resolve_manifest() {
    "$RESOLVE" "$1"
}

# ── helper: resolve a profile into a temp file ────────────────────────────────
resolve_to_file() {
    local profile="$1"
    local tmpfile
    tmpfile=$(mktemp "$TMPDIR_ROOT/resolved.XXXXXX.yaml")
    "$RESOLVE" "$LOADOUTS_DIR/$profile" > "$tmpfile"
    echo "$tmpfile"
}

# ==============================================================================
echo ""
echo "=== Test: base-minimal profile ==="
# ==============================================================================
OUTDIR_MINIMAL="$TMPDIR_ROOT/minimal"
RESOLVED_MINIMAL=$(resolve_to_file "profiles/basic.yaml")
run_generate "$RESOLVED_MINIMAL" "$OUTDIR_MINIMAL" "full" "false"
USERDATA="$OUTDIR_MINIMAL/user-data"

assert_exits_ok "user-data file exists" test -f "$USERDATA"
assert_contains  "has cloud-config header"         "#cloud-config"                       "$USERDATA"
assert_contains  "hostname set"                    "hostname: $VM_NAME"                  "$USERDATA"
assert_contains  "agent user present"              "name: agent"                         "$USERDATA"
assert_contains  "user SSH key injected"           "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKEYEXAMPLE" "$USERDATA"
assert_contains  "ephemeral SSH key injected"      "AAAAIEPHEMERALEXAMPLE"               "$USERDATA"
assert_contains  "package_update: true"            "package_update: true"                "$USERDATA"
assert_contains  "health server written"           "health-server.py"                    "$USERDATA"
assert_contains  "health-token written"            "/etc/agentic-sandbox/health-token"   "$USERDATA"
assert_contains  "health service written"          "agentic-health.service"              "$USERDATA"
assert_contains  "agent service written"           "agentic-agent.service"               "$USERDATA"
assert_contains  "agent.env written"               "/etc/agentic-sandbox/agent.env"      "$USERDATA"
assert_contains  "check-ready.sh written"          "check-ready.sh"                      "$USERDATA"
assert_contains  "install.sh written"              "/opt/agentic-setup/install.sh"       "$USERDATA"
assert_contains  "welcome message written"         "99-agentic-welcome.sh"               "$USERDATA"
assert_contains  "bootstrap token env written"     "AGENT_BOOTSTRAP_TOKEN=bootstrap-token-not-real" "$USERDATA"
assert_not_contains "legacy agent secret env omitted" "AGENT_SECRET="                    "$USERDATA"
assert_not_contains "legacy agent secret value omitted" "$AGENT_SECRET"                  "$USERDATA"
assert_contains  "health token substituted"        "$HEALTH_TOKEN"                       "$USERDATA"
assert_contains  "vm name in agent service"        "$VM_NAME"                            "$USERDATA"
assert_contains  "management server substituted"   "$MGMT_SERVER"                        "$USERDATA"
assert_contains  "runcmd section present"          "runcmd:"                             "$USERDATA"
assert_contains  "UFW configured"                  "ufw enable"                          "$USERDATA"
assert_contains  "qemu-guest-agent started"        "systemctl start qemu-guest-agent"    "$USERDATA"
assert_contains  "health server started"           "systemctl start agentic-health"      "$USERDATA"
assert_contains  "install.sh launched"             "nohup /opt/agentic-setup/install.sh" "$USERDATA"
assert_contains  "setup-complete marker"           "agentic-setup-complete"              "$USERDATA"
assert_not_contains "no raw PLACEHOLDER tokens remain" "PLACEHOLDER"                     "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: insecure loadout rejects legacy agent secret fallback ==="
# ==============================================================================
OUTDIR_INSECURE="$TMPDIR_ROOT/insecure-rejected"
if run_generate_insecure "$RESOLVED_MINIMAL" "$OUTDIR_INSECURE" "full" "false" \
      2>"$TMPDIR_ROOT/insecure-rejected.err"; then
    fail "insecure loadout should reject legacy secret fallback"
else
    assert_contains "insecure loadout reports legacy retirement" \
        "legacy AGENT_SECRET loadout fallback was retired in #412" \
        "$TMPDIR_ROOT/insecure-rejected.err"
fi

# ==============================================================================
echo ""
echo "=== Test: secure mTLS loadout omits legacy agent secret ==="
# ==============================================================================
OUTDIR_TLS="$TMPDIR_ROOT/secure-mtls"
run_generate_tls "$RESOLVED_MINIMAL" "$OUTDIR_TLS" "full" "false"
USERDATA="$OUTDIR_TLS/user-data"

assert_contains "secure transport defaults to auto" "AGENT_TRANSPORT=auto" "$USERDATA"
assert_contains "TLS CA path written"               "AGENT_GRPC_TLS_CA=/etc/agentic-sandbox/grpc-mtls/ca.pem" "$USERDATA"
assert_contains "TLS directory created early"        "mkdir -p /etc/agentic-sandbox/grpc-mtls" "$USERDATA"
assert_contains "TLS parent traversable by agent"    "chmod 0750 /etc/agentic-sandbox" "$USERDATA"
assert_contains "TLS cert/key staged as root-owned"  "owner: root:root" "$USERDATA"
assert_contains "TLS cert/key chowned for agent"     "chown agent:agent /etc/agentic-sandbox/grpc-mtls/agent.pem /etc/agentic-sandbox/grpc-mtls/agent-key.pem" "$USERDATA"
assert_contains "TLS cert mode is group-readable"    "permissions: '0640'" "$USERDATA"
assert_contains "TLS key mode is private"            "permissions: '0600'" "$USERDATA"
assert_contains "TLS cert material written"         "test-cert" "$USERDATA"
assert_contains "TLS key material written"          "test-key" "$USERDATA"
assert_contains "bootstrap token env written"        "AGENT_BOOTSTRAP_TOKEN=bootstrap-token-not-real" "$USERDATA"
assert_contains "bootstrap SPIFFE env written"       "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" "$USERDATA"
assert_contains "bootstrap expiry env written"       "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=1900000000000" "$USERDATA"
assert_not_contains "secure loadout omits AGENT_SECRET env" "AGENT_SECRET=" "$USERDATA"
assert_not_contains "secure loadout omits secret CLI arg" "--secret" "$USERDATA"
assert_not_contains "secure loadout omits legacy secret value" "$AGENT_SECRET" "$USERDATA"
assert_not_contains "secure loadout leaves no placeholders" "PLACEHOLDER" "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: bootstrap enrollment loadout omits legacy agent secret ==="
# ==============================================================================
OUTDIR_BOOTSTRAP="$TMPDIR_ROOT/bootstrap-enrollment"
run_generate_bootstrap "$RESOLVED_MINIMAL" "$OUTDIR_BOOTSTRAP" "full" "false"
USERDATA="$OUTDIR_BOOTSTRAP/user-data"

assert_contains "bootstrap token env written"        "AGENT_BOOTSTRAP_TOKEN=bootstrap-token-not-real" "$USERDATA"
assert_contains "bootstrap SPIFFE env written"       "AGENT_BOOTSTRAP_SPIFFE_ID=spiffe://sandbox.agentic.local/agent/018fb9f1-3291-7a73-b261-c7de8a2af4d1" "$USERDATA"
assert_contains "bootstrap expiry env written"       "AGENT_BOOTSTRAP_TOKEN_EXPIRES_AT_UNIX_MS=1900000000000" "$USERDATA"
assert_not_contains "bootstrap loadout omits AGENT_SECRET env" "AGENT_SECRET=" "$USERDATA"
assert_not_contains "bootstrap loadout omits secret CLI arg" "--secret" "$USERDATA"
assert_not_contains "bootstrap loadout omits legacy secret value" "$AGENT_SECRET" "$USERDATA"
assert_not_contains "bootstrap loadout leaves no placeholders" "PLACEHOLDER" "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: agentshare mounts ==="
# ==============================================================================
OUTDIR_AGENTSHARE="$TMPDIR_ROOT/agentshare"
run_generate "$RESOLVED_MINIMAL" "$OUTDIR_AGENTSHARE" "full" "true"
USERDATA="$OUTDIR_AGENTSHARE/user-data"

assert_contains "virtiofs fstab entries"    "virtiofs"              "$USERDATA"
assert_contains "global mount point"        "/mnt/global"           "$USERDATA"
assert_contains "inbox mount point"         "/mnt/inbox"            "$USERDATA"
assert_contains "outbox mount point"        "/mnt/outbox"           "$USERDATA"
assert_contains "global symlink"            "/home/agent/global"    "$USERDATA"
assert_contains "inbox symlink"             "/home/agent/inbox"     "$USERDATA"
assert_contains "workspace symlink"         "/home/agent/workspace" "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: browser-qa carbonyl sessions ==="
# ==============================================================================
RESOLVED_BROWSER_QA=$(resolve_to_file "profiles/browser-qa.yaml")
OUTDIR_BROWSER_QA="$TMPDIR_ROOT/browser-qa"
run_generate "$RESOLVED_BROWSER_QA" "$OUTDIR_BROWSER_QA" "full" "true"
USERDATA="$OUTDIR_BROWSER_QA/user-data"

assert_contains "carbonyl session target created" "/home/agent/.local/share/carbonyl-agent/sessions" "$USERDATA"
assert_contains "carbonyl session fstab entry" "carbonylsessions /home/agent/.local/share/carbonyl-agent/sessions virtiofs rw,noatime,nofail 0 0" "$USERDATA"
assert_contains "carbonyl session mount command" "mount -t virtiofs carbonylsessions" "$USERDATA"
assert_contains "carbonyl session directory mode" "chmod 700 /home/agent/.local/share/carbonyl-agent/sessions" "$USERDATA"
assert_yaml_ok "browser-qa user-data parses as YAML" "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: network_mode isolated ==="
# ==============================================================================
OUTDIR_ISOLATED="$TMPDIR_ROOT/isolated"
run_generate "$RESOLVED_MINIMAL" "$OUTDIR_ISOLATED" "isolated" "false"
USERDATA="$OUTDIR_ISOLATED/user-data"

assert_contains "isolated mode token in runcmd" "isolated"         "$USERDATA"
assert_contains "deny outgoing rule present"    "deny outgoing"    "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: network_mode allowlist ==="
# ==============================================================================
OUTDIR_ALLOWLIST="$TMPDIR_ROOT/allowlist"
run_generate "$RESOLVED_MINIMAL" "$OUTDIR_ALLOWLIST" "allowlist" "false"
USERDATA="$OUTDIR_ALLOWLIST/user-data"

assert_contains "allowlist mode token"          "allowlist"        "$USERDATA"
assert_contains "external DNS blocked"          "Block external DNS" "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: agentic-dev profile (docker + runtimes + ai_tools) ==="
# ==============================================================================
RESOLVED_AGENTIC=$(resolve_to_file "profiles/agentic-dev.yaml")
OUTDIR_AGENTIC="$TMPDIR_ROOT/agentic-dev"
run_generate "$RESOLVED_AGENTIC" "$OUTDIR_AGENTIC" "full" "false"
USERDATA="$OUTDIR_AGENTIC/user-data"

assert_contains "docker rootless setup script"  "setup-rootless-docker.sh"         "$USERDATA"
assert_contains "bashrc-additions present"       "bashrc-additions.sh"              "$USERDATA"
assert_contains "setup-user-tools present"       "setup-user-tools.sh"              "$USERDATA"
assert_contains "uv installer in user-tools"     "astral.sh/uv/install.sh"          "$USERDATA"
assert_contains "fnm installer in user-tools"    "fnm.vercel.app/install"           "$USERDATA"
assert_contains "rust installer in user-tools"   "sh.rustup.rs"                     "$USERDATA"
assert_contains "go installation in install.sh"  "go.dev/dl"                        "$USERDATA"
assert_contains "claude code installer"          "claude.ai/install.sh"             "$USERDATA"
assert_contains "managed-settings written"       "/etc/claude-code/managed-settings.json" "$USERDATA"
assert_contains "aider config written"           ".aider.conf.yml"                  "$USERDATA"
assert_contains "codex config written"           ".codex/config.toml"               "$USERDATA"
assert_contains "GOPATH set in bashrc"           "GOPATH"                           "$USERDATA"
assert_contains "fnm env in bashrc"              "fnm env"                          "$USERDATA"
assert_contains "docker host in bashrc"          "DOCKER_HOST"                      "$USERDATA"
assert_contains "aiwg use command present"       "aiwg use all"           "$USERDATA"
assert_not_contains "no raw PLACEHOLDER tokens remain" "PLACEHOLDER"                "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: claude-only profile ==="
# ==============================================================================
RESOLVED_CLAUDE=$(resolve_to_file "profiles/claude-only.yaml")
OUTDIR_CLAUDE="$TMPDIR_ROOT/claude-only"
run_generate "$RESOLVED_CLAUDE" "$OUTDIR_CLAUDE" "full" "false"
USERDATA="$OUTDIR_CLAUDE/user-data"

assert_contains  "claude code enabled"           "claude.ai/install.sh"   "$USERDATA"
# claude-only extends base-dev (rust) + docker, so these ARE expected
assert_contains  "docker included via base-dev"  "setup-rootless-docker"  "$USERDATA"
assert_contains  "rust included via base-dev"    "sh.rustup.rs"           "$USERDATA"
# aider is NOT part of claude-only (no ai_tools.aider)
assert_not_contains "no aider config"            "aider.conf.yml"         "$USERDATA"

# ==============================================================================
echo ""
echo "=== Test: startup credential refs are metadata only ==="
# ==============================================================================
CREDENTIAL_REF_MANIFEST="$TMPDIR_ROOT/credential-refs.yaml"
cat > "$CREDENTIAL_REF_MANIFEST" <<'CREDYAML'
apiVersion: loadout/v1
kind: loadout
metadata:
  name: credential-ref-test
  description: Credential ref policy test
extends:
  - layers/base-minimal.yaml
credential_refs:
  - id: cred_anthropic_api
    provider: claude
    allowed_use: provider_api
    required: true
    target:
      type: env
      name: ANTHROPIC_API_KEY
  - id: cred_git_ssh
    provider: git
    allowed_use: git_ssh
    required: false
    target:
      type: file
      name: git_ssh_key
CREDYAML

RESOLVED_CREDENTIAL_REFS="$TMPDIR_ROOT/resolved-credential-refs.yaml"
"$RESOLVE" "$CREDENTIAL_REF_MANIFEST" > "$RESOLVED_CREDENTIAL_REFS"

OUTDIR_CREDENTIAL_REFS="$TMPDIR_ROOT/credential-refs"
run_generate "$RESOLVED_CREDENTIAL_REFS" "$OUTDIR_CREDENTIAL_REFS" "full" "false"
USERDATA="$OUTDIR_CREDENTIAL_REFS/user-data"

assert_contains "credential refs policy written" "/etc/agentic-sandbox/credential-refs.json" "$USERDATA"
assert_contains "credential refs env points at policy" "AGENTIC_CREDENTIAL_REFS=/etc/agentic-sandbox/credential-refs.json" "$USERDATA"
assert_contains "credential dir env written" "AGENTIC_CREDENTIAL_DIR=/run/agentic-sandbox/credentials" "$USERDATA"
assert_contains "credential runtime dir prepared" "install -d -m 0700 -o agent -g agent /run/agentic-sandbox/credentials" "$USERDATA"
assert_contains "credential id included" "\"id\": \"cred_anthropic_api\"" "$USERDATA"
assert_contains "credential allowed use included" "\"allowed_use\": \"provider_api\"" "$USERDATA"
assert_contains "credential target hint included" "\"name\": \"ANTHROPIC_API_KEY\"" "$USERDATA"
assert_not_contains "no credential secret value embedded" "sk-ant-" "$USERDATA"
assert_yaml_ok "credential-ref user-data parses as YAML" "$USERDATA"

BAD_CREDENTIAL_REF_MANIFEST="$TMPDIR_ROOT/bad-credential-refs.yaml"
cat > "$BAD_CREDENTIAL_REF_MANIFEST" <<'BADCREDYAML'
apiVersion: loadout/v1
kind: loadout
metadata:
  name: bad-credential-ref-test
  description: Credential ref rejection test
extends:
  - layers/base-minimal.yaml
credential_refs:
  - id: cred_bad
    provider: claude
    allowed_use: provider_api
    value: sk-ant-not-real
    target:
      type: env
      name: ANTHROPIC_API_KEY
BADCREDYAML

RESOLVED_BAD_CREDENTIAL_REFS="$TMPDIR_ROOT/resolved-bad-credential-refs.yaml"
"$RESOLVE" "$BAD_CREDENTIAL_REF_MANIFEST" > "$RESOLVED_BAD_CREDENTIAL_REFS"
OUTDIR_BAD_CREDENTIAL_REFS="$TMPDIR_ROOT/bad-credential-refs"
if run_generate "$RESOLVED_BAD_CREDENTIAL_REFS" "$OUTDIR_BAD_CREDENTIAL_REFS" "full" "false" \
      2>"$TMPDIR_ROOT/bad-credential-refs.err"; then
    fail "credential refs should reject inline secret values"
else
    assert_contains "credential refs reject secret-like field" \
        "credential_refs[0] contains secret-like field(s): value" \
        "$TMPDIR_ROOT/bad-credential-refs.err"
fi

# ==============================================================================
echo ""
echo "=== Test: GPU passthrough config ==="
# ==============================================================================
# Create a temporary manifest with GPU enabled
GPU_MANIFEST="$TMPDIR_ROOT/gpu-manifest.yaml"
cat > "$GPU_MANIFEST" <<'GPUYAML'
apiVersion: loadout/v1
kind: loadout
metadata:
  name: gpu-test
  description: GPU passthrough test
extends:
  - layers/base-minimal.yaml
resources:
  cpus: 4
  memory: 8G
  disk: 40G
  gpu:
    enabled: true
    device: "0000:41:00.0"
    driver: vfio-pci
GPUYAML

RESOLVED_GPU="$TMPDIR_ROOT/resolved-gpu.yaml"
"$RESOLVE" "$GPU_MANIFEST" > "$RESOLVED_GPU"

OUTDIR_GPU="$TMPDIR_ROOT/gpu"
run_generate "$RESOLVED_GPU" "$OUTDIR_GPU" "full" "false"
USERDATA="$OUTDIR_GPU/user-data"

assert_contains  "GPU driver install in runcmd"     "ubuntu-drivers"     "$USERDATA"
assert_exits_ok  "gpu-config sidecar exists"        test -f "$OUTDIR_GPU/gpu-config"
assert_contains  "gpu-config has enabled=true"      "GPU_ENABLED=true"   "$OUTDIR_GPU/gpu-config"
assert_contains  "gpu-config has PCI device"        "0000:41:00.0"       "$OUTDIR_GPU/gpu-config"
assert_contains  "gpu-config has driver"            "GPU_DRIVER=vfio-pci" "$OUTDIR_GPU/gpu-config"

# Also verify non-GPU profiles do NOT produce gpu-config
assert_not_contains "basic has no GPU driver runcmd" "ubuntu-drivers" "$OUTDIR_MINIMAL/user-data"

# ==============================================================================
echo ""
echo "=== Test: automation-control profile ==="
# ==============================================================================
RESOLVED_AUTOMATION=$(resolve_to_file "profiles/automation-control.yaml")
OUTDIR_AUTOMATION="$TMPDIR_ROOT/automation-control"
run_generate "$RESOLVED_AUTOMATION" "$OUTDIR_AUTOMATION" "full" "false"
USERDATA="$OUTDIR_AUTOMATION/user-data"

assert_contains  "automation control helper installed" "agentic-provider-inventory" "$USERDATA"
assert_contains  "provider readiness helper installed" "agentic-provider-readiness" "$USERDATA"
assert_contains  "claude automation helper installed" "agentic-claude-automation" "$USERDATA"
assert_contains  "github automation helper installed" "agentic-github-automation" "$USERDATA"
assert_contains  "ssh automation helper installed" "agentic-ssh-automation" "$USERDATA"
assert_contains  "codex launcher prefers key file" "OPENAI_API_KEY_FILE" "$USERDATA"
assert_contains  "claude launcher prefers key file" "ANTHROPIC_API_KEY_FILE" "$USERDATA"
assert_contains  "github launcher uses askpass helper" "GIT_ASKPASS" "$USERDATA"
assert_contains  "ssh launcher uses git ssh command" "GIT_SSH_COMMAND" "$USERDATA"
assert_contains  "provider home isolation supported" "AGENTIC_PROVIDER_HOME" "$USERDATA"
assert_contains  "automation control note written" "/etc/agentic-sandbox/automation-control.md" "$USERDATA"
assert_contains  "codex config included" ".codex/config.toml" "$USERDATA"
assert_contains  "ops framework deployed" "aiwg use ops" "$USERDATA"
assert_contains  "sdlc framework deployed" "aiwg use sdlc" "$USERDATA"
assert_not_contains "no credential value in automation helper" "sk-test" "$USERDATA"

FAKE_BIN="$TMPDIR_ROOT/provider-bin"
FAKE_CREDS="$TMPDIR_ROOT/provider-creds"
mkdir -p "$FAKE_BIN" "$FAKE_CREDS"
printf '%s\n' 'sk-test-readiness-secret' > "$FAKE_CREDS/openai_api_key"
cat > "$FAKE_BIN/codex" <<'SH'
#!/usr/bin/env bash
if [ "${1:-}" = "--version" ]; then
  printf 'codex-cli 1.2.3\n'
  exit 0
fi
exit 0
SH
chmod +x "$FAKE_BIN/codex"
READINESS_OUT="$TMPDIR_ROOT/provider-readiness.out"
PATH="$FAKE_BIN:$PATH" \
AGENTIC_CREDENTIAL_DIR="$FAKE_CREDS" \
  "$LOADOUTS_DIR/../../common/automation-control/provider-readiness.sh" codex > "$READINESS_OUT"
assert_contains "readiness emits schema" "schema	agentic.provider_readiness.v1" "$READINESS_OUT"
assert_contains "readiness reports present credential" "codex	codex	present	codex-cli 1.2.3	present_unvalidated	none" "$READINESS_OUT"
assert_not_contains "readiness redacts credential value" "sk-test-readiness-secret" "$READINESS_OUT"

# ==============================================================================
echo ""
echo "=== Test: browser-qa readiness budget ==="
# ==============================================================================
RESOLVED_BROWSER_QA=$(resolve_to_file "profiles/browser-qa.yaml")
assert_contains "browser-qa setup wait budget" "setup_timeout_seconds: 1200" "$RESOLVED_BROWSER_QA"

# ==============================================================================
echo ""
echo "=== Test: argument validation ==="
# ==============================================================================
TMP_OUT="$TMPDIR_ROOT/argcheck"
mkdir -p "$TMP_OUT"

if "$GENERATE" 2>/dev/null; then
    fail "should exit non-zero with no args"
else
    pass "exits non-zero with no args"
fi

if "$GENERATE" "/nonexistent/path.yaml" "$VM_NAME" "$SSH_KEY" "$TMP_OUT" \
       "false" "$AGENT_SECRET" "$EPHEMERAL_KEY" "$MAC_ADDRESS" "full" "$HEALTH_TOKEN" \
       2>/dev/null; then
    fail "should exit non-zero with missing manifest"
else
    pass "exits non-zero when manifest does not exist"
fi

# ==============================================================================
echo ""
echo "=== Summary ==="
echo "  Passed: $PASS"
echo "  Failed: $FAIL"
if [[ $FAIL -gt 0 ]]; then
    echo ""
    echo "Failed assertions:"
    for e in "${ERRORS[@]}"; do
        echo "  - $e"
    done
    exit 1
fi
echo "All tests passed."

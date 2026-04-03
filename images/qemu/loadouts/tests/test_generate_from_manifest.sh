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
    if grep -qF "$needle" "$file"; then
        pass "$label"
    else
        fail "$label (expected to find: $needle)"
    fi
}

assert_not_contains() {
    local label="$1" needle="$2" file="$3"
    if ! grep -qF "$needle" "$file"; then
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
assert_contains  "agent secret substituted"        "$AGENT_SECRET"                       "$USERDATA"
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
assert_contains "aiwg use command present"       "aiwg use sdlc-complete"           "$USERDATA"
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

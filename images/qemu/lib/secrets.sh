#!/bin/bash
# lib/secrets.sh - Ephemeral secret and SSH key management for agent VMs
#
# Provides functions to generate, retrieve, and revoke:
#   - Agent authentication secrets (256-bit, SHA256 hash stored on host)
#   - Health endpoint tokens
#   - Ephemeral SSH key pairs for automated access
#
# Required globals (validated at source time):
#   SECRETS_DIR          - Host directory for secrets storage
#   AGENT_TOKENS_FILE    - Path to agent-tokens text file
#   HEALTH_TOKENS_FILE   - Path to health-tokens text file

: "${SECRETS_DIR:?lib/secrets.sh requires SECRETS_DIR}"
: "${AGENT_TOKENS_FILE:?lib/secrets.sh requires AGENT_TOKENS_FILE}"
: "${HEALTH_TOKENS_FILE:?lib/secrets.sh requires HEALTH_TOKENS_FILE}"

# Generate ephemeral secret for agent authentication
# Returns the plaintext secret (256-bit hex) and stores the hash
# Writes to both agent-tokens (legacy) and agent-hashes.json (management server format)
generate_agent_secret() {
    local agent_id="$1"

    # Ensure secrets directory exists — readable by management server user
    sudo mkdir -p "$SECRETS_DIR"
    sudo chmod 755 "$SECRETS_DIR"
    sudo touch "$AGENT_TOKENS_FILE"
    sudo chmod 644 "$AGENT_TOKENS_FILE"

    # Generate 256-bit (32 bytes) random secret
    local secret
    secret=$(openssl rand -hex 32)

    # Compute SHA256 hash of the secret
    local secret_hash
    secret_hash=$(echo -n "$secret" | sha256sum | cut -d' ' -f1)

    # Remove any existing entry for this agent
    sudo sed -i "/^${agent_id}:/d" "$AGENT_TOKENS_FILE" 2>/dev/null || true

    # Store agent_id:hash in text format (legacy)
    echo "${agent_id}:${secret_hash}" | sudo tee -a "$AGENT_TOKENS_FILE" > /dev/null

    # Update agent-hashes.json (the format the management server reads)
    local hashes_file="$SECRETS_DIR/agent-hashes.json"
    if [[ -f "$hashes_file" ]]; then
        # Merge into existing JSON
        python3 -c "
import json
with open('$hashes_file') as f:
    data = json.load(f)
data['$agent_id'] = '$secret_hash'
with open('$hashes_file', 'w') as f:
    json.dump(data, f, indent=2)
"
    else
        # Create new JSON file
        echo "{\"$agent_id\": \"$secret_hash\"}" | python3 -m json.tool | sudo tee "$hashes_file" > /dev/null
    fi
    sudo chmod 644 "$hashes_file"

    # Return the plaintext secret (to inject into cloud-init)
    echo "$secret"
}

# Get secret hash for an agent (for display/verification only)
get_agent_secret_hash() {
    local agent_id="$1"
    grep "^${agent_id}:" "$AGENT_TOKENS_FILE" 2>/dev/null | cut -d: -f2
}

# Revoke an agent's secret (from both storage formats)
revoke_agent_secret() {
    local agent_id="$1"
    # Remove from text file
    sudo sed -i "/^${agent_id}:/d" "$AGENT_TOKENS_FILE" 2>/dev/null || true
    # Remove from JSON file
    local hashes_file="$SECRETS_DIR/agent-hashes.json"
    if [[ -f "$hashes_file" ]]; then
        python3 -c "
import json
with open('$hashes_file') as f:
    data = json.load(f)
data.pop('$agent_id', None)
with open('$hashes_file', 'w') as f:
    json.dump(data, f, indent=2)
" 2>/dev/null || true
    fi
}

# Generate health endpoint authentication token
# Token stored on VM at /etc/agentic-sandbox/health-token
# Hash stored on host for management server verification
generate_health_token() {
    local agent_id="$1"

    # Ensure health tokens file exists
    sudo mkdir -p "$SECRETS_DIR"
    sudo touch "$HEALTH_TOKENS_FILE"
    sudo chmod 644 "$HEALTH_TOKENS_FILE"

    # Generate 256-bit random token
    local token
    token=$(openssl rand -hex 32)

    # Compute SHA256 hash
    local token_hash
    token_hash=$(echo -n "$token" | sha256sum | cut -d' ' -f1)

    # Remove existing entry
    sudo sed -i "/^${agent_id}:/d" "$HEALTH_TOKENS_FILE" 2>/dev/null || true

    # Store agent_id:hash
    echo "${agent_id}:${token_hash}" | sudo tee -a "$HEALTH_TOKENS_FILE" > /dev/null

    # Return plaintext token for injection
    echo "$token"
}

# Get health token hash for verification
get_health_token_hash() {
    local agent_id="$1"
    grep "^${agent_id}:" "$HEALTH_TOKENS_FILE" 2>/dev/null | cut -d: -f2
}

# Revoke health token
revoke_health_token() {
    local agent_id="$1"
    sudo sed -i "/^${agent_id}:/d" "$HEALTH_TOKENS_FILE" 2>/dev/null || true
}

# Generate ephemeral SSH key pair for automated access
# Private key stored on host for management processes
# Public key injected into VM for SSH access
generate_agent_ssh_key() {
    local agent_id="$1"
    local key_dir="$SECRETS_DIR/ssh-keys"
    local private_key="$key_dir/${agent_id}"
    local public_key="$key_dir/${agent_id}.pub"

    # Ensure key directory exists with secure permissions
    sudo mkdir -p "$key_dir"
    sudo chmod 700 "$key_dir"

    # Remove existing keys for this agent
    sudo rm -f "$private_key" "$public_key" 2>/dev/null || true

    # Generate ed25519 key pair (no passphrase for automation)
    sudo ssh-keygen -t ed25519 -N "" -C "agentic-sandbox:${agent_id}" -f "$private_key" -q

    # Secure permissions
    sudo chmod 600 "$private_key"
    sudo chmod 644 "$public_key"

    # Return public key content
    sudo cat "$public_key"
}

# Get path to agent's ephemeral SSH private key
get_agent_ssh_key_path() {
    local agent_id="$1"
    echo "$SECRETS_DIR/ssh-keys/${agent_id}"
}

# Revoke agent's ephemeral SSH key pair
revoke_agent_ssh_key() {
    local agent_id="$1"
    local key_dir="$SECRETS_DIR/ssh-keys"
    sudo rm -f "$key_dir/${agent_id}" "$key_dir/${agent_id}.pub" 2>/dev/null || true
}

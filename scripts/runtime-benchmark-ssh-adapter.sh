#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 VM_NAME [USER]" >&2
  exit 2
fi

vm_name="$1"
remote_user="${2:-agent}"
vm_root="${VM_STORAGE_DIR:-/var/lib/agentic-sandbox/vms}"
vm_info="$vm_root/$vm_name/vm-info.json"

[[ "$vm_name" =~ ^[a-z][a-z0-9-]{1,62}$ ]] || {
  echo "invalid benchmark VM name" >&2
  exit 2
}
[[ "$remote_user" =~ ^[a-z_][a-z0-9_-]*$ ]] || {
  echo "invalid benchmark VM user" >&2
  exit 2
}
[[ -r "$vm_info" ]] || {
  echo "benchmark VM metadata is unavailable" >&2
  exit 1
}

ip="$(jq -er '.ip | select(type == "string")' "$vm_info")"
private_key="$(jq -er '.management.ssh_key_path | select(type == "string")' "$vm_info")"
[[ "$ip" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] || {
  echo "benchmark VM metadata has an invalid address" >&2
  exit 1
}
[[ -e "$private_key" ]] || {
  echo "ephemeral benchmark SSH key is unavailable" >&2
  exit 1
}

ssh_command=(ssh)
if [[ ! -r "$private_key" ]]; then
  ssh_command=(sudo -n ssh)
fi

exec "${ssh_command[@]}" \
  -o BatchMode=yes \
  -o ConnectTimeout=5 \
  -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null \
  -i "$private_key" \
  "$remote_user@$ip" \
  python3 -

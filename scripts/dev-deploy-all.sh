#!/usr/bin/env bash
#
# Full development deploy: rebuild server + agent, restart, deploy to all running VMs
# Usage: ./scripts/dev-deploy-all.sh [--debug]
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

DEBUG_FLAG=""
[[ "${1:-}" == "--debug" ]] && DEBUG_FLAG="--debug"

echo "=== Rebuilding Management Server ==="
cd "$PROJECT_ROOT/management"
./dev.sh restart

echo ""
echo "=== Rebuilding Agent ==="
cd "$PROJECT_ROOT/agent-rs"
cargo build --release 2>&1 | tail -3

echo ""
echo "=== Deploying to Running VMs ==="
for vm in $(virsh list --name 2>/dev/null | grep -E "^agent-"); do
    echo "Deploying to $vm..."
    "$SCRIPT_DIR/deploy-agent.sh" "$vm" $DEBUG_FLAG || echo "  Failed to deploy to $vm"
done

echo ""
echo "=== Done ==="
curl -s http://localhost:8122/api/v1/agents | python3 -c "import sys,json; d=json.load(sys.stdin); print(f\"Connected agents: {len(d.get('agents',[]))}\")" 2>/dev/null || echo "Server not responding"

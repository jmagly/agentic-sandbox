#!/bin/bash
set -e

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

echo "=== E2E Integration Test Runner ==="
echo ""

# 1. Build management server
echo "[1/4] Building management server (release)..."
cd "$REPO_ROOT/management" && cargo build --release
echo "      -> $(ls -1 target/release/agentic-mgmt)"

# 2. Build Rust agent
echo "[2/4] Building Rust agent (release)..."
cd "$REPO_ROOT/agent-rs" && cargo build --release
echo "      -> $(ls -1 target/release/agent-client)"

# 3. Set up Python environment
echo "[3/4] Installing Python test dependencies..."
cd "$REPO_ROOT"
if [ -d ".venv" ]; then
    source .venv/bin/activate
fi
pip install -q -r "$REPO_ROOT/tests/e2e/requirements.txt"

# 4. Run tests
echo "[4/4] Running E2E tests..."
echo ""
cd "$REPO_ROOT"
python -m pytest tests/e2e/ -v --tb=short -x "$@"

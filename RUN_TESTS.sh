#!/bin/bash
# Test runner for Agentic Sandbox (Rust + Python)
set -e

echo "==================================="
echo "Agentic Sandbox Test Suite"
echo "==================================="
echo ""

# Check for Rust
if ! command -v cargo &> /dev/null; then
    echo "Error: Rust (cargo) is not installed or not in PATH"
    echo "Install Rust from https://rustup.rs/"
    exit 1
fi

echo "Rust version:"
rustc --version
cargo --version
echo ""

echo "Running Rust unit tests..."
( cd management && cargo test )
( cd agent-rs && cargo test )
( cd cli && cargo test )
echo ""

if command -v python &> /dev/null; then
    echo "Running Python SDK tests..."
    ( cd sdk/python && python -m pytest )
    echo ""
else
    echo "Python not found; skipping Python SDK tests"
fi

echo "==================================="
echo "Test suite complete!"
echo "==================================="

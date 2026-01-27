#!/bin/bash
# Test runner for Go sandbox manager
set -e

echo "==================================="
echo "Agentic Sandbox Test Suite"
echo "==================================="
echo ""

# Check for Go
if ! command -v go &> /dev/null; then
    echo "Error: Go is not installed or not in PATH"
    echo "Install Go 1.23+ from https://go.dev/dl/"
    exit 1
fi

echo "Go version:"
go version
echo ""

# Download dependencies
echo "Downloading dependencies..."
go mod download
echo ""

# Run tests
echo "Running tests..."
go test -v -race ./...
echo ""

# Generate coverage
echo "Generating coverage report..."
go test -coverprofile=coverage.out ./...
go tool cover -func=coverage.out
echo ""

echo "Coverage report saved to coverage.html"
go tool cover -html=coverage.out -o coverage.html

echo ""
echo "==================================="
echo "Test suite complete!"
echo "==================================="

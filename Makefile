.PHONY: build test lint clean docker help install deps integration-test test-unit test-integration test-e2e test-all coverage coverage-check

# Build variables
BINARY_DIR := bin
MANAGER_BINARY := $(BINARY_DIR)/sandbox-manager
CLI_BINARY := $(BINARY_DIR)/sandbox-cli
GO_FILES := $(shell find . -type f -name '*.go' -not -path "./vendor/*")

# Docker image tags
BASE_IMAGE := agentic-sandbox-base:latest
TEST_IMAGE := agentic-sandbox-test:latest

help: ## Show this help message
	@echo 'Usage: make [target]'
	@echo ''
	@echo 'Available targets:'
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

deps: ## Install Go dependencies
	@echo "Installing Go dependencies..."
	go mod download
	go mod verify

install-tools: ## Install development tools
	@echo "Installing development tools..."
	go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest

# Go build targets
build: build-manager build-cli ## Build all binaries

build-manager: deps ## Build sandbox-manager binary
	@echo "Building sandbox-manager..."
	@mkdir -p $(BINARY_DIR)
	go build -o $(MANAGER_BINARY) ./cmd/sandbox-manager

build-cli: deps ## Build sandbox-cli binary
	@echo "Building sandbox-cli..."
	@mkdir -p $(BINARY_DIR)
	go build -o $(CLI_BINARY) ./cmd/sandbox-cli

# Test targets
test: test-unit ## Run unit tests (default)

test-unit: ## Run unit tests only
	@echo "Running unit tests..."
	go test -v -race -timeout 5m ./internal/...

test-integration: ## Run integration tests (requires Docker)
	@echo "Running integration tests..."
	go test -tags=integration -v -timeout 10m ./tests/integration/... ./internal/runtime/...

test-all: test-unit test-integration ## Run all tests (unit + integration)
	@echo "All tests completed"

test-coverage: ## Run tests with coverage report
	@echo "Running tests with coverage..."
	go test -tags=integration -coverprofile=coverage.out -covermode=atomic ./...
	go tool cover -html=coverage.out -o coverage.html
	@echo "Coverage report generated: coverage.html"

coverage: test-coverage ## Alias for test-coverage
	go tool cover -func=coverage.out
	@echo ""
	@go tool cover -func=coverage.out | grep total | awk '{print "Total coverage: " $$3}'

coverage-check: coverage ## Check coverage meets 80% target
	@echo "Checking coverage meets 80% target..."
	@coverage=$$(go tool cover -func=coverage.out | grep total | awk '{print $$3}' | sed 's/%//'); \
	if [ $$(echo "$$coverage < 80" | bc) -eq 1 ]; then \
		echo "ERROR: Coverage $$coverage% is below 80% target"; \
		exit 1; \
	else \
		echo "SUCCESS: Coverage $$coverage% meets target"; \
	fi

test-docker: ## Run Docker adapter tests only
	@echo "Running Docker adapter tests..."
	go test -tags=integration -v -timeout 10m -run TestDocker ./internal/runtime/... ./tests/integration/...

test-qemu: ## Run QEMU adapter tests only
	@echo "Running QEMU adapter tests..."
	go test -tags=integration -v -timeout 10m -run TestQEMU ./internal/runtime/...

test-e2e: ## Run E2E integration tests (management server + agents)
	@echo "Running E2E integration tests..."
	./scripts/run-e2e-tests.sh

test-api: ## Run API handler tests only
	@echo "Running API handler tests..."
	go test -v -timeout 5m ./internal/api/...

lint: ## Run linter
	@echo "Running linter..."
	golangci-lint run --timeout 5m

fmt: ## Format Go code
	@echo "Formatting code..."
	go fmt ./...

vet: ## Run go vet
	@echo "Running go vet..."
	go vet ./...

# Docker targets
docker: docker-base docker-test ## Build all Docker images

docker-base: ## Build base Docker image
	@echo "Building base Docker image..."
	docker build -t $(BASE_IMAGE) images/base/

docker-test: ## Build test Docker image
	@echo "Building test Docker image..."
	docker build -t $(TEST_IMAGE) images/test/

docker-clean: ## Remove Docker images
	@echo "Removing Docker images..."
	docker rmi -f $(BASE_IMAGE) $(TEST_IMAGE) 2>/dev/null || true

# Integration tests
integration-test: test-integration ## Alias for test-integration

# Development environment
dev-setup: ## Set up development environment
	@echo "Setting up development environment..."
	./scripts/dev-setup.sh

dev-up: ## Start development environment
	@echo "Starting development environment..."
	docker-compose -f docker-compose.dev.yaml up -d

dev-down: ## Stop development environment
	@echo "Stopping development environment..."
	docker-compose -f docker-compose.dev.yaml down

dev-logs: ## View development environment logs
	docker-compose -f docker-compose.dev.yaml logs -f

# Clean targets
clean: ## Remove build artifacts
	@echo "Cleaning build artifacts..."
	rm -rf $(BINARY_DIR)/
	rm -f coverage.out coverage.html
	go clean -testcache

clean-all: clean docker-clean ## Remove all build artifacts and Docker images

# Install binaries to system
install: build ## Install binaries to /usr/local/bin
	@echo "Installing binaries..."
	install -m 755 $(MANAGER_BINARY) /usr/local/bin/
	install -m 755 $(CLI_BINARY) /usr/local/bin/
	@echo "Installed to /usr/local/bin/"

uninstall: ## Uninstall binaries from system
	@echo "Uninstalling binaries..."
	rm -f /usr/local/bin/sandbox-manager
	rm -f /usr/local/bin/sandbox-cli

# Quick checks before commit
check: fmt vet lint test-all coverage-check ## Run all checks (format, vet, lint, test, coverage)

.DEFAULT_GOAL := help

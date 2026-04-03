.PHONY: build test lint clean docker help install deps integration-test test-unit test-integration test-e2e test-all fmt vet check build-agent-musl build-agent-all

# Docker image tags
BASE_IMAGE := agentic-sandbox-base:latest
TEST_IMAGE := agentic-sandbox-test:latest

help: ## Show this help message
	@echo 'Usage: make [target]'
	@echo ''
	@echo 'Available targets:'
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2}'

# Build targets
build: ## Build Rust components (management server, agent client, CLI)
	@echo "Building Rust components..."
	@$(MAKE) -s build-management
	@$(MAKE) -s build-agent
	@$(MAKE) -s build-cli

build-management: ## Build Rust management server
	@echo "Building management server..."
	@cd management && cargo build --release

build-agent: ## Build Rust agent client (glibc - for Ubuntu)
	@echo "Building agent client (glibc)..."
	@cd agent-rs && cargo build --release

build-agent-musl: ## Build agent client (musl/static - for Alpine)
	@echo "Building agent client (musl/static)..."
	@cd agent-rs && cargo build --release --target x86_64-unknown-linux-musl --no-default-features

build-agent-all: build-agent build-agent-musl ## Build both agent variants (glibc + musl)

build-cli: ## Build Rust CLI
	@echo "Building CLI..."
	@cd cli && cargo build --release

# Test targets
test: test-unit ## Run unit tests (default)

test-unit: ## Run Rust unit tests
	@echo "Running Rust unit tests..."
	@cd management && cargo test
	@cd agent-rs && cargo test
	@cd cli && cargo test

# E2E tests
test-e2e: ## Run E2E integration tests (management server + agents)
	@echo "Running E2E integration tests..."
	./scripts/run-e2e-tests.sh

test-all: test-unit test-e2e ## Run all tests
	@echo "All tests completed"

# Lint/format targets
fmt: ## Format Rust code
	@echo "Formatting Rust code..."
	@cd management && cargo fmt
	@cd agent-rs && cargo fmt
	@cd cli && cargo fmt

lint: ## Check Rust formatting
	@echo "Checking Rust formatting..."
	@cd management && cargo fmt -- --check
	@cd agent-rs && cargo fmt -- --check
	@cd cli && cargo fmt -- --check

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
	@cd management && cargo clean
	@cd agent-rs && cargo clean
	@cd cli && cargo clean

clean-all: clean docker-clean ## Remove all build artifacts and Docker images

# Quick checks before commit
check: lint test-unit ## Run all checks (format check + tests)

.DEFAULT_GOAL := help

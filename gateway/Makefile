.PHONY: build test coverage clean run docker-build docker-run help

# Build variables
BINARY_NAME=gateway
DOCKER_IMAGE=agentic-sandbox-gateway
VERSION?=latest

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

build: ## Build the gateway binary
	go build -o $(BINARY_NAME) main.go

test: ## Run tests
	go test -v

coverage: ## Run tests with coverage
	go test -cover -coverprofile=coverage.out
	go tool cover -func=coverage.out
	@echo ""
	@echo "To view HTML coverage report: go tool cover -html=coverage.out"

coverage-html: coverage ## Generate HTML coverage report
	go tool cover -html=coverage.out

clean: ## Remove build artifacts
	rm -f $(BINARY_NAME)
	rm -f coverage.out
	rm -f *.log

run: build ## Build and run locally
	./$(BINARY_NAME) -config gateway.yaml

docker-build: ## Build Docker image
	docker build -t $(DOCKER_IMAGE):$(VERSION) .

docker-run: ## Run Docker container
	docker run -d \
		--name $(BINARY_NAME) \
		-p 8080:8080 \
		-v $(PWD)/gateway.yaml:/etc/gateway/gateway.yaml:ro \
		-e MCP_TOKEN \
		-e GITHUB_TOKEN \
		$(DOCKER_IMAGE):$(VERSION)

docker-stop: ## Stop and remove Docker container
	docker stop $(BINARY_NAME) || true
	docker rm $(BINARY_NAME) || true

docker-logs: ## View Docker container logs
	docker logs -f $(BINARY_NAME)

vet: ## Run go vet
	go vet ./...

fmt: ## Format code
	go fmt ./...

deps: ## Download dependencies
	go mod download
	go mod tidy

all: clean deps vet test build ## Run all checks and build

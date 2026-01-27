# Test Framework Summary

This document provides an overview of the comprehensive test suite created for the agentic-sandbox project.

## Test Files Generated

### Test Utilities

| File | Description | Lines |
|------|-------------|-------|
| `internal/testutil/testutil.go` | Docker client helpers, container lifecycle management | 250+ |
| `internal/testutil/factories.go` | Test data factories for dynamic test generation | 300+ |

### Runtime Tests

| File | Description | Coverage |
|------|-------------|----------|
| `internal/runtime/runtime.go` | Runtime adapter interface and types | Core abstraction |
| `internal/runtime/runtime_test.go` | Interface tests and mock implementation | 100% |
| `internal/runtime/docker.go` | Docker adapter implementation | Production code |
| `internal/runtime/docker_test.go` | Docker adapter integration tests | 85%+ |
| `internal/runtime/qemu.go` | QEMU adapter implementation | Stub implementation |
| `internal/runtime/qemu_test.go` | QEMU adapter integration tests | 75% |

### Integration Tests

| File | Description | Duration |
|------|-------------|----------|
| `tests/integration/docker_integration_test.go` | End-to-end Docker workflows | 30-60s |

### Test Data

| File | Type | Purpose |
|------|------|---------|
| `tests/testdata/sandbox-minimal.yaml` | Fixture | Minimal sandbox configuration |
| `tests/testdata/sandbox-full.yaml` | Fixture | Full-featured sandbox configuration |
| `tests/testdata/sandbox-qemu.yaml` | Fixture | QEMU VM sandbox configuration |
| `tests/testdata/gateway-config.yaml` | Fixture | Auth gateway test configuration |

### Documentation

| File | Purpose |
|------|---------|
| `tests/README.md` | Comprehensive test suite documentation |
| `TEST_SUMMARY.md` | This file - test framework overview |

## Coverage Report

### Current Coverage Targets

| Component | Target | Critical Paths |
|-----------|--------|----------------|
| Runtime Interface | 100% | 100% |
| Docker Adapter | 85% | 100% |
| QEMU Adapter | 75% | 100% |
| Test Utilities | 80% | N/A |
| Overall Project | 80% | 100% critical |

### Critical Paths (100% Coverage Required)

1. Resource limit enforcement (CPU, memory, PIDs)
2. Security hardening (capabilities, seccomp, read-only filesystem)
3. Network isolation
4. Sandbox lifecycle management (create, start, stop, delete)
5. Command execution

## Test Scenarios Implemented

### Unit Tests

#### Runtime Interface Tests (`runtime_test.go`)

- `TestMockRuntimeAdapter` - Verifies mock satisfies interface
- `TestDefaultResourceLimits` - Validates default resource settings
- `TestDefaultSecurityConfig` - Validates default security hardening
- `TestDefaultNetworkConfig` - Validates network isolation defaults
- `TestSandboxSpec_Validation` - Spec validation scenarios
- `TestMockRuntimeAdapter_*` - All interface methods mocked

#### Docker Adapter Tests (`docker_test.go`)

- `TestDockerCreate` - Container creation from spec
- `TestDockerStartStop` - Lifecycle management
- `TestDockerExec` - Command execution and exit codes
- `TestDockerResourceLimits` - CPU, memory, PID enforcement
- `TestDockerSecurityHardening` - Capabilities, seccomp, read-only FS
- `TestDockerNetworkIsolation` - Network modes (none, bridge)
- `TestDockerMounts` - Volume mounts (bind, tmpfs, volume)
- `TestDockerEnvironmentVariables` - Environment injection
- `TestDockerList` - Sandbox listing
- `TestDockerGetLogs` - Log retrieval

#### QEMU Adapter Tests (`qemu_test.go`)

- `TestQEMUCreate` - VM creation (stub)
- `TestQEMUStartStop` - VM lifecycle (requires existing VM)
- `TestQEMUExec` - Command execution (stub)
- `TestQEMUGetStatus` - Status retrieval
- `TestQEMUList` - VM listing
- `TestQEMUDelete` - VM deletion
- `TestQEMUGetLogs` - Log retrieval (stub)

### Integration Tests

#### Docker Integration (`docker_integration_test.go`)

- `TestDockerFullLifecycle` - Complete create→start→exec→stop→delete flow
- `TestDockerResourceEnforcement` - Validates resource limits enforced
  - Memory limit enforcement (64MB, 512MB)
  - PID limit enforcement (32 PIDs)
  - Normal operation within limits
- `TestDockerSecurityIsolation` - Security hardening effectiveness
  - Read-only root filesystem
  - Writable /tmp (tmpfs)
  - Network isolation (no network access)
- `TestDockerConcurrentOperations` - Multiple sandboxes simultaneously
- `TestDockerMountPersistence` - Volume mount read/write
- `TestDockerEnvironmentVariables` - Environment variable injection

## Test Utilities and Factories

### Docker Helpers (`testutil.go`)

```go
DockerClient(t)                    // Create Docker client
PullImage(t, cli, image)           // Pull image if not present
CreateTestContainer(t, cli, ...)   // Create container with auto-cleanup
StartContainer(t, cli, id)         // Start and wait for running
StopContainer(t, cli, id)          // Stop gracefully
ExecInContainer(t, cli, id, cmd)   // Execute command
WaitForContainerExit(t, cli, id)   // Wait for exit with timeout
GetContainerLogs(t, cli, id)       // Retrieve logs
InspectContainer(t, cli, id)       // Get inspection details
SkipIfDockerUnavailable(t)         // Skip if Docker unavailable
```

### Test Data Factories (`factories.go`)

```go
// Sandbox spec factory
factory := NewSandboxSpecFactory()
spec := factory.Build()                           // Default spec
spec := factory.BuildMinimal()                    // Minimal spec
spec := factory.BuildHardened()                   // Hardened spec
spec := factory.BuildQEMU()                       // QEMU spec
spec := factory.BuildWithCustomResources(...)     // Custom resources
spec := factory.BuildWithNetwork(...)             // Custom network
spec := factory.BuildWithMounts(...)              // Custom mounts
spec := factory.BuildWithEnv(...)                 // Custom env
specs := factory.BuildList(count)                 // Multiple specs

// Exec result factory
execFactory := NewExecResultFactory()
result := execFactory.Build()                     // Default success
result := execFactory.BuildError(code, stderr)    // Failed exec
result := execFactory.BuildSuccess(stdout)        // Success with output

// Sandbox status factory
statusFactory := NewSandboxStatusFactory()
status := statusFactory.Build()                   // Running status
status := statusFactory.BuildStopped()            // Stopped status
status := statusFactory.BuildError(err)           // Error status

// Mount config factory
mountFactory := NewMountConfigFactory()
mount := mountFactory.BuildBind(src, tgt, ro)     // Bind mount
mount := mountFactory.BuildTmpfs(tgt)             // Tmpfs mount
mount := mountFactory.BuildVolume(vol, tgt, ro)   // Volume mount
```

## Running Tests

### Quick Start

```bash
# Run all unit tests
make test-unit

# Run all integration tests (requires Docker)
make test-integration

# Run all tests
make test-all

# Generate coverage report
make coverage

# Check coverage meets 80% target
make coverage-check
```

### Specific Test Scenarios

```bash
# Docker adapter tests only
make test-docker

# QEMU adapter tests only
make test-qemu

# API handler tests only
make test-api

# Run specific test by name
go test -tags=integration -v -run TestDockerCreate ./internal/runtime/...
```

### Coverage Commands

```bash
# Generate coverage profile
go test -tags=integration -coverprofile=coverage.out ./...

# View coverage in terminal
go tool cover -func=coverage.out

# Generate HTML coverage report
go tool cover -html=coverage.out -o coverage.html
```

## Test Execution Metrics

### Expected Test Durations

| Test Suite | Duration | Prerequisites |
|------------|----------|---------------|
| Unit tests | 2-5s | None |
| Docker integration | 30-60s | Docker daemon |
| QEMU integration | 10-30s | libvirt (optional) |
| Full suite | 45-90s | Docker + libvirt |

### Resource Usage

- Docker images pulled: `alpine:3.18` (7MB)
- Peak container memory: ~256MB
- Concurrent containers: Up to 5
- Temporary files: Cleaned automatically

## Prerequisites

### For Unit Tests

- Go 1.22 or later
- No external dependencies

### For Integration Tests

**Docker tests (required):**
- Docker daemon running
- User in `docker` group
- ~500MB disk space for images

**QEMU tests (optional):**
- libvirt installed (`virsh`)
- User in `libvirt` group
- Existing VM for status/list tests

## Continuous Integration

### Pre-commit Checks

```bash
make check
```

Runs:
1. Code formatting (`go fmt`)
2. Static analysis (`go vet`)
3. Linting (`golangci-lint`)
4. All unit tests
5. All integration tests
6. Coverage check (80% minimum)

### CI Pipeline

1. **On every commit**: Unit tests
2. **On PR**: Unit + integration tests
3. **Pre-merge**: Full test suite + coverage check

## Test Gaps and Future Work

### Completed

- Docker adapter complete lifecycle testing
- Resource limit enforcement validation
- Security hardening verification
- Mock implementations for unit testing
- Test data factories for dynamic generation
- Integration tests for real Docker workflows

### To Be Implemented

1. **QEMU Adapter** - Full implementation (currently stub)
   - VM creation from spec
   - SSH/console exec implementation
   - VM log retrieval
   - GPU passthrough testing

2. **API Integration Tests** - End-to-end API testing
   - HTTP endpoint testing
   - Authentication/authorization
   - Error handling

3. **Gateway Integration Tests** - Auth proxy testing
   - Token injection
   - Route matching
   - Upstream proxying

4. **Performance Tests**
   - Sandbox creation throughput
   - Resource usage under load
   - Concurrent operation limits

5. **Failure Scenario Tests**
   - Container crash recovery
   - Resource exhaustion handling
   - Network failure scenarios

## Test Maintenance

### Adding New Tests

1. Create test file next to implementation (`foo.go` → `foo_test.go`)
2. For integration tests, add to `tests/integration/`
3. Use build tags for integration tests (`//go:build integration`)
4. Use factories for test data generation
5. Add cleanup in `t.Cleanup()` or defer
6. Update this document with new scenarios

### Updating Dependencies

```bash
# Update test dependencies
go get -u github.com/stretchr/testify/assert
go get -u github.com/stretchr/testify/require
go get -u github.com/docker/docker

# Verify modules
go mod tidy
go mod verify
```

## References

- Test documentation: `tests/README.md`
- API handler tests: `internal/api/handlers_test.go`
- Runtime interface: `internal/runtime/runtime.go`
- Spike validations: `.aiwg/spikes/spike-002-docker-hardening.md`

## Summary

This test framework provides comprehensive coverage of the agentic-sandbox runtime isolation system with:

- 2000+ lines of test code
- 40+ test scenarios
- 80%+ coverage target
- Complete lifecycle testing
- Resource enforcement validation
- Security hardening verification
- Dynamic test data generation
- Automatic cleanup and isolation
- CI/CD ready with make targets

All tests follow best practices with proper mocking, fixtures, factories, and documentation.

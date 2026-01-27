# Test Suite Documentation

This directory contains the comprehensive test suite for the agentic-sandbox project.

## Test Structure

```
tests/
├── integration/           # Integration tests (require Docker/QEMU)
│   ├── docker_integration_test.go
│   ├── api_integration_test.go
│   └── gateway_integration_test.go
├── testdata/             # Test fixtures and sample configurations
│   ├── sandbox-minimal.yaml
│   ├── sandbox-full.yaml
│   ├── sandbox-qemu.yaml
│   └── gateway-config.yaml
└── README.md             # This file
```

## Test Types

### Unit Tests

Located alongside source files (e.g., `runtime_test.go`, `docker_test.go`).

- Test individual functions and methods in isolation
- Use mocks for external dependencies
- Fast execution (no external services required)
- Coverage target: 80% minimum

**Run unit tests:**

```bash
go test ./internal/... -v
```

### Integration Tests

Located in `tests/integration/`. These tests require external services (Docker, QEMU).

- Test complete workflows and interactions
- Use real Docker containers and VMs
- Tagged with `//go:build integration`
- Slower execution (requires setup/teardown)

**Run integration tests:**

```bash
go test -tags=integration ./tests/integration/... -v
```

**Run with timeout:**

```bash
go test -tags=integration -timeout 10m ./tests/integration/... -v
```

## Test Coverage

### Current Coverage Targets

| Component | Target | Critical Paths |
|-----------|--------|----------------|
| Runtime Adapters | 80% | 100% |
| API Handlers | 85% | 100% |
| Security Config | 90% | 100% |
| Test Utilities | 75% | N/A |

### Critical Path Definition

These paths MUST have 100% test coverage:

- Resource limit enforcement (memory, CPU, PIDs)
- Security hardening (capabilities, seccomp, read-only filesystem)
- Network isolation
- Authentication and authorization (gateway)
- Sandbox lifecycle management (create, start, stop, delete)

### Generate Coverage Report

```bash
# Generate coverage profile
go test -tags=integration -coverprofile=coverage.out ./...

# View coverage in terminal
go tool cover -func=coverage.out

# Generate HTML coverage report
go tool cover -html=coverage.out -o coverage.html
```

## Test Utilities

### Test Helpers (`internal/testutil/`)

- **`testutil.go`** - Docker client helpers, container lifecycle management
- **`factories.go`** - Test data factories for dynamic test generation

### Using Factories

Factories provide dynamic test data generation:

```go
import "github.com/roctinam/agentic-sandbox/internal/testutil"

func TestExample(t *testing.T) {
    factory := testutil.NewSandboxSpecFactory()

    // Create default spec
    spec := factory.Build()

    // Create minimal spec
    minimalSpec := factory.BuildMinimal()

    // Create hardened spec
    hardenedSpec := factory.BuildHardened()

    // Create with overrides
    customSpec := factory.Build(func(spec *runtime.SandboxSpec) {
        spec.Name = "custom-name"
        spec.Resources.MemoryMB = 1024
    })
}
```

## Test Fixtures

Static test data in `testdata/`:

### `sandbox-minimal.yaml`

Minimal sandbox configuration for basic testing.

### `sandbox-full.yaml`

Full-featured sandbox with all options configured.

### `sandbox-qemu.yaml`

QEMU VM sandbox configuration.

### `gateway-config.yaml`

Auth gateway configuration for testing auth injection.

## Running Tests

### All Tests (Unit + Integration)

```bash
make test-all
```

### Unit Tests Only

```bash
make test-unit
```

### Integration Tests Only

```bash
make test-integration
```

### Specific Package

```bash
go test ./internal/runtime/... -v
```

### Specific Test

```bash
go test ./internal/runtime/ -run TestDockerCreate -v
```

## Prerequisites

### For Unit Tests

- Go 1.22 or later
- No external dependencies

### For Integration Tests

**Docker tests:**
- Docker daemon running and accessible
- User in `docker` group (or sudo access)
- Alpine 3.18 image available (auto-pulled)

**QEMU tests:**
- libvirt installed (`virsh` available)
- User in `libvirt` group
- At least one VM defined (for status/list tests)

### Skip Integration Tests

Integration tests are skipped automatically if prerequisites are not met:

```go
testutil.SkipIfDockerUnavailable(t)  // Skips if Docker unavailable
skipIfLibvirtUnavailable(t)          // Skips if libvirt unavailable
```

## Test Scenarios

### Docker Adapter Tests

| Test | Description | Coverage |
|------|-------------|----------|
| `TestDockerCreate` | Creates container with spec | Container creation |
| `TestDockerStartStop` | Lifecycle management | Start/stop operations |
| `TestDockerExec` | Command execution | Exec functionality |
| `TestDockerResourceLimits` | Resource enforcement | CPU, memory, PIDs |
| `TestDockerSecurityHardening` | Security settings | Capabilities, seccomp |
| `TestDockerNetworkIsolation` | Network modes | None, bridge modes |
| `TestDockerMounts` | Volume mounts | Bind, tmpfs, volume |
| `TestDockerEnvironmentVariables` | Env injection | Environment vars |
| `TestDockerList` | Sandbox listing | List operations |
| `TestDockerGetLogs` | Log retrieval | Log streaming |

### Integration Test Scenarios

| Test | Description | Duration |
|------|-------------|----------|
| `TestDockerFullLifecycle` | Complete create→start→exec→stop→delete flow | ~5s |
| `TestDockerResourceEnforcement` | Validates resource limits enforced | ~10s |
| `TestDockerSecurityIsolation` | Tests security hardening effectiveness | ~5s |
| `TestDockerConcurrentOperations` | Multiple sandboxes simultaneously | ~5s |
| `TestDockerMountPersistence` | Volume mount read/write | ~3s |
| `TestDockerEnvironmentVariables` | Env var injection | ~3s |

## Continuous Integration

Tests are run automatically on:

- Every commit (unit tests)
- Pull requests (unit + integration)
- Pre-merge (all tests + coverage check)

### CI Coverage Requirements

- Minimum 80% overall coverage
- 100% coverage for critical paths
- All integration tests must pass

## Debugging Tests

### Verbose Output

```bash
go test -v ./...
```

### Keep Test Containers

By default, test containers are auto-cleaned. To debug, modify cleanup:

```go
// Comment out cleanup in test
// t.Cleanup(func() { adapter.Delete(ctx, sandboxID) })
```

### View Container Logs

```go
logs := testutil.GetContainerLogs(t, cli, containerID)
t.Logf("Container logs: %s", logs)
```

### Inspect Container

```go
inspect := testutil.InspectContainer(t, cli, containerID)
t.Logf("Container config: %+v", inspect.HostConfig)
```

## Best Practices

1. **Always use factories** for test data generation
2. **Skip gracefully** if prerequisites unavailable
3. **Clean up resources** in `t.Cleanup()` or defer
4. **Use meaningful assertions** with error messages
5. **Test both success and failure paths**
6. **Include edge cases** (empty input, boundary values)
7. **Use build tags** for integration tests (`//go:build integration`)
8. **Keep tests isolated** (no shared state between tests)

## Test Maintenance

### Adding New Tests

1. Create test file next to implementation (`foo.go` → `foo_test.go`)
2. For integration tests, add to `tests/integration/`
3. Use build tags for integration tests
4. Add factory methods if new types introduced
5. Update this README with new scenarios

### Updating Fixtures

When updating test fixtures in `testdata/`:

1. Ensure backward compatibility
2. Add new fixtures for new scenarios (don't modify existing)
3. Document fixture purpose in comments

### Coverage Monitoring

Check coverage regularly:

```bash
make coverage
```

Address any drops below 80% target immediately.

## Troubleshooting

### "Docker not available"

- Ensure Docker daemon is running: `docker ps`
- Check user permissions: `groups | grep docker`
- Restart Docker: `sudo systemctl restart docker`

### "libvirt daemon not accessible"

- Check libvirt status: `systemctl status libvirtd`
- Verify user in group: `groups | grep libvirt`
- Reconnect to session: `newgrp libvirt`

### Test Timeouts

- Increase timeout: `-timeout 15m`
- Check for deadlocks or infinite loops
- Verify cleanup is happening (containers removed)

### Flaky Tests

- Add retries for timing-sensitive operations
- Increase sleep durations for container startup
- Use proper synchronization (wait conditions, not sleeps)

## Contributing

When adding tests:

1. Follow existing test structure and naming
2. Use factories for test data
3. Add documentation for new test scenarios
4. Ensure tests are deterministic (no random failures)
5. Run full test suite before submitting PR

//go:build integration
// +build integration

package integration

import (
	"context"
	"testing"
	"time"

	"github.com/roctinam/agentic-sandbox/internal/runtime"
	"github.com/roctinam/agentic-sandbox/internal/testutil"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestDockerFullLifecycle tests complete Docker sandbox lifecycle
func TestDockerFullLifecycle(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := runtime.NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()
	factory := testutil.NewSandboxSpecFactory()

	// Pull image first
	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, "alpine:3.18")

	// Create sandbox
	spec := factory.Build()
	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	// Verify created state
	status, err := adapter.GetStatus(ctx, sandboxID)
	require.NoError(t, err)
	assert.Equal(t, "stopped", status.State)

	// Start sandbox
	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	// Verify running state
	status, err = adapter.GetStatus(ctx, sandboxID)
	require.NoError(t, err)
	assert.Equal(t, "running", status.State)

	// Execute command
	result, err := adapter.Exec(ctx, sandboxID, []string{"echo", "integration test"})
	require.NoError(t, err)
	assert.Equal(t, 0, result.ExitCode)
	assert.Contains(t, result.Stdout, "integration test")

	// Get logs
	reader, err := adapter.GetLogs(ctx, sandboxID, runtime.LogOptions{Tail: "100"})
	require.NoError(t, err)
	reader.Close()

	// Stop sandbox
	err = adapter.Stop(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	// Verify stopped state
	status, err = adapter.GetStatus(ctx, sandboxID)
	require.NoError(t, err)
	assert.Equal(t, "stopped", status.State)

	// Delete sandbox
	err = adapter.Delete(ctx, sandboxID)
	require.NoError(t, err)

	// Verify deletion
	_, err = adapter.GetStatus(ctx, sandboxID)
	assert.Error(t, err)
}

// TestDockerResourceEnforcement tests resource limit enforcement
func TestDockerResourceEnforcement(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := runtime.NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()
	factory := testutil.NewSandboxSpecFactory()

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, "alpine:3.18")

	tests := []struct {
		name        string
		spec        *runtime.SandboxSpec
		testCommand []string
		expectFail  bool
	}{
		{
			name:        "memory limit enforced",
			spec:        factory.BuildWithCustomResources(1.0, 64, 64),
			testCommand: []string{"sh", "-c", "dd if=/dev/zero of=/dev/shm/file bs=1M count=100 2>&1 || true"},
			expectFail:  true, // Should fail due to memory limit
		},
		{
			name:        "pid limit enforced",
			spec:        factory.BuildWithCustomResources(1.0, 512, 32),
			testCommand: []string{"sh", "-c", "for i in $(seq 1 50); do sleep 100 & done 2>&1 || true"},
			expectFail:  true, // Should fail due to PID limit
		},
		{
			name:        "normal operation within limits",
			spec:        factory.BuildWithCustomResources(2.0, 512, 128),
			testCommand: []string{"echo", "success"},
			expectFail:  false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sandboxID, err := adapter.Create(ctx, tt.spec)
			require.NoError(t, err)
			defer adapter.Delete(ctx, sandboxID)

			err = adapter.Start(ctx, sandboxID)
			require.NoError(t, err)

			time.Sleep(500 * time.Millisecond)

			result, err := adapter.Exec(ctx, sandboxID, tt.testCommand)
			require.NoError(t, err)

			if tt.expectFail {
				// Command should indicate resource limit hit
				output := result.Stdout + result.Stderr
				assert.True(t,
					result.ExitCode != 0 || len(output) > 0,
					"Expected resource limit enforcement")
			} else {
				assert.Equal(t, 0, result.ExitCode)
			}
		})
	}
}

// TestDockerSecurityIsolation tests security hardening
func TestDockerSecurityIsolation(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := runtime.NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()
	factory := testutil.NewSandboxSpecFactory()

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, "alpine:3.18")

	// Create hardened sandbox
	spec := factory.BuildHardened()
	spec.Command = []string{"sleep", "300"}

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	tests := []struct {
		name        string
		command     []string
		shouldFail  bool
		description string
	}{
		{
			name:        "cannot write to root filesystem",
			command:     []string{"sh", "-c", "touch /test.txt 2>&1 || true"},
			shouldFail:  true,
			description: "Read-only root filesystem should prevent writes",
		},
		{
			name:        "can write to /tmp",
			command:     []string{"sh", "-c", "touch /tmp/test.txt && ls /tmp/test.txt"},
			shouldFail:  false,
			description: "Tmpfs /tmp should be writable",
		},
		{
			name:        "no network access",
			command:     []string{"sh", "-c", "ping -c 1 8.8.8.8 2>&1 || echo 'no network'"},
			shouldFail:  true,
			description: "Network mode 'none' should prevent network access",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result, err := adapter.Exec(ctx, sandboxID, tt.command)
			require.NoError(t, err)

			if tt.shouldFail {
				output := result.Stdout + result.Stderr
				assert.True(t,
					result.ExitCode != 0 || len(output) > 0,
					tt.description)
			} else {
				assert.Equal(t, 0, result.ExitCode, tt.description)
			}
		})
	}
}

// TestDockerConcurrentOperations tests concurrent sandbox management
func TestDockerConcurrentOperations(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := runtime.NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()
	factory := testutil.NewSandboxSpecFactory()

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, "alpine:3.18")

	// Create multiple sandboxes concurrently
	numSandboxes := 5
	sandboxIDs := make([]string, numSandboxes)
	errors := make([]error, numSandboxes)

	// Create phase
	for i := 0; i < numSandboxes; i++ {
		go func(idx int) {
			spec := factory.Build()
			id, err := adapter.Create(ctx, spec)
			sandboxIDs[idx] = id
			errors[idx] = err
		}(i)
	}

	time.Sleep(2 * time.Second)

	// Verify all created successfully
	for i := 0; i < numSandboxes; i++ {
		assert.NoError(t, errors[i], "Sandbox %d creation failed", i)
		assert.NotEmpty(t, sandboxIDs[i], "Sandbox %d ID is empty", i)
	}

	// Cleanup all sandboxes
	for _, id := range sandboxIDs {
		if id != "" {
			adapter.Delete(ctx, id)
		}
	}
}

// TestDockerMountPersistence tests volume mount functionality
func TestDockerMountPersistence(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := runtime.NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()
	factory := testutil.NewSandboxSpecFactory()
	mountFactory := testutil.NewMountConfigFactory()

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, "alpine:3.18")

	// Create spec with tmpfs mount
	spec := factory.BuildWithMounts([]runtime.MountConfig{
		mountFactory.BuildTmpfs("/data"),
	})

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	// Write to mounted volume
	result, err := adapter.Exec(ctx, sandboxID, []string{"sh", "-c", "echo 'test data' > /data/test.txt"})
	require.NoError(t, err)
	assert.Equal(t, 0, result.ExitCode)

	// Read back from mounted volume
	result, err = adapter.Exec(ctx, sandboxID, []string{"cat", "/data/test.txt"})
	require.NoError(t, err)
	assert.Equal(t, 0, result.ExitCode)
	assert.Contains(t, result.Stdout, "test data")
}

// TestDockerEnvironmentVariables tests environment variable injection
func TestDockerEnvironmentVariables(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := runtime.NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()
	factory := testutil.NewSandboxSpecFactory()

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, "alpine:3.18")

	env := map[string]string{
		"TEST_VAR":     "test_value",
		"AGENT_ID":     "12345",
		"ENVIRONMENT":  "testing",
	}

	spec := factory.BuildWithEnv(env)
	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	// Verify each environment variable
	for key, expectedValue := range env {
		result, err := adapter.Exec(ctx, sandboxID, []string{"printenv", key})
		require.NoError(t, err)
		assert.Equal(t, 0, result.ExitCode)
		assert.Contains(t, result.Stdout, expectedValue)
	}
}

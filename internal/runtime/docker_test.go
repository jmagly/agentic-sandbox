//go:build integration
// +build integration

package runtime

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/docker/docker/api/types/container"
	"github.com/roctinam/agentic-sandbox/internal/testutil"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestDockerCreate(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-create-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Command:   []string{"sleep", "300"},
	}

	// Pull image first
	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	// Create sandbox
	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	require.NotEmpty(t, sandboxID)

	// Cleanup
	defer adapter.Delete(ctx, sandboxID)

	// Verify container exists
	status, err := adapter.GetStatus(ctx, sandboxID)
	require.NoError(t, err)
	assert.Equal(t, "stopped", status.State) // Created but not started
	assert.Equal(t, "docker", status.Runtime)
}

func TestDockerStartStop(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-lifecycle-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Command:   []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	// Start container
	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	// Wait a moment for container to start
	time.Sleep(500 * time.Millisecond)

	// Verify running
	status, err := adapter.GetStatus(ctx, sandboxID)
	require.NoError(t, err)
	assert.Equal(t, "running", status.State)

	// Stop container
	err = adapter.Stop(ctx, sandboxID)
	require.NoError(t, err)

	// Verify stopped
	time.Sleep(500 * time.Millisecond)
	status, err = adapter.GetStatus(ctx, sandboxID)
	require.NoError(t, err)
	assert.Equal(t, "stopped", status.State)
}

func TestDockerExec(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-exec-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Command:   []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	// Execute command
	result, err := adapter.Exec(ctx, sandboxID, []string{"echo", "hello world"})
	require.NoError(t, err)
	assert.Equal(t, 0, result.ExitCode)
	assert.Contains(t, result.Stdout, "hello world")

	// Execute failing command
	result, err = adapter.Exec(ctx, sandboxID, []string{"sh", "-c", "exit 42"})
	require.NoError(t, err)
	assert.Equal(t, 42, result.ExitCode)
}

func TestDockerResourceLimits(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:    "test-resources-sandbox",
		Image:   "alpine:3.18",
		Runtime: "docker",
		Resources: ResourceLimits{
			CPUs:      2.0,
			MemoryMB:  256, // 256MB
			PIDsLimit: 64,
		},
		Network:  DefaultNetworkConfig(),
		Security: DefaultSecurityConfig(),
		Command:  []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	// Inspect container to verify limits
	inspect := testutil.InspectContainer(t, cli, sandboxID)

	// Verify CPU limit
	expectedCPUs := int64(2.0 * 1e9) // NanoCPUs
	assert.Equal(t, expectedCPUs, inspect.HostConfig.NanoCPUs)

	// Verify memory limit
	expectedMemory := int64(256 * 1024 * 1024) // bytes
	assert.Equal(t, expectedMemory, inspect.HostConfig.Memory)

	// Verify PID limit
	assert.NotNil(t, inspect.HostConfig.PidsLimit)
	assert.Equal(t, int64(64), *inspect.HostConfig.PidsLimit)
}

func TestDockerSecurityHardening(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-security-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security: SecurityConfig{
			Privileged:      false,
			ReadOnlyRootFS:  true,
			NoNewPrivileges: true,
			CapDrop:         []string{"ALL"},
			CapAdd:          []string{},
		},
		Command: []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	// Inspect container to verify security settings
	inspect := testutil.InspectContainer(t, cli, sandboxID)

	// Verify not privileged
	assert.False(t, inspect.HostConfig.Privileged)

	// Verify read-only root
	assert.True(t, inspect.HostConfig.ReadonlyRootfs)

	// Verify capabilities dropped
	assert.Contains(t, inspect.HostConfig.CapDrop, "ALL")

	// Verify no-new-privileges
	assert.Contains(t, inspect.HostConfig.SecurityOpt, "no-new-privileges:true")

	// Verify /tmp is writable (tmpfs mount)
	assert.NotNil(t, inspect.HostConfig.Tmpfs)
	assert.Contains(t, inspect.HostConfig.Tmpfs, "/tmp")
}

func TestDockerNetworkIsolation(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	tests := []struct {
		name        string
		networkMode string
	}{
		{"none - no network", "none"},
		{"bridge - isolated bridge", "bridge"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			spec := &SandboxSpec{
				Name:      "test-network-sandbox-" + tt.networkMode,
				Image:     "alpine:3.18",
				Runtime:   "docker",
				Resources: DefaultResourceLimits(),
				Network: NetworkConfig{
					Mode: tt.networkMode,
				},
				Security: DefaultSecurityConfig(),
				Command:  []string{"sleep", "300"},
			}

			cli := testutil.DockerClient(t)
			testutil.PullImage(t, cli, spec.Image)

			sandboxID, err := adapter.Create(ctx, spec)
			require.NoError(t, err)
			defer adapter.Delete(ctx, sandboxID)

			// Verify network mode
			inspect := testutil.InspectContainer(t, cli, sandboxID)
			assert.Equal(t, container.NetworkMode(tt.networkMode), inspect.HostConfig.NetworkMode)
		})
	}
}

func TestDockerMounts(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-mounts-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Mounts: []MountConfig{
			{
				Type:     "tmpfs",
				Target:   "/data",
				ReadOnly: false,
			},
		},
		Command: []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	// Verify mounts
	inspect := testutil.InspectContainer(t, cli, sandboxID)
	assert.NotEmpty(t, inspect.HostConfig.Mounts)

	// Find the /data mount
	foundMount := false
	for _, m := range inspect.HostConfig.Mounts {
		if m.Target == "/data" {
			foundMount = true
			assert.Equal(t, "tmpfs", string(m.Type))
			break
		}
	}
	assert.True(t, foundMount, "Expected /data mount not found")
}

func TestDockerEnvironmentVariables(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-env-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Env: map[string]string{
			"TEST_VAR":  "test_value",
			"AGENT_ID":  "12345",
		},
		Command: []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	time.Sleep(500 * time.Millisecond)

	// Verify environment variables
	result, err := adapter.Exec(ctx, sandboxID, []string{"printenv", "TEST_VAR"})
	require.NoError(t, err)
	assert.Equal(t, 0, result.ExitCode)
	assert.Contains(t, result.Stdout, "test_value")
}

func TestDockerList(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	// Create multiple sandboxes
	spec1 := &SandboxSpec{
		Name:      "test-list-sandbox-1",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Command:   []string{"sleep", "300"},
	}

	spec2 := &SandboxSpec{
		Name:      "test-list-sandbox-2",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Command:   []string{"sleep", "300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec1.Image)

	id1, err := adapter.Create(ctx, spec1)
	require.NoError(t, err)
	defer adapter.Delete(ctx, id1)

	id2, err := adapter.Create(ctx, spec2)
	require.NoError(t, err)
	defer adapter.Delete(ctx, id2)

	// List sandboxes
	list, err := adapter.List(ctx)
	require.NoError(t, err)

	// Verify our sandboxes are in the list
	found := 0
	for _, info := range list {
		if strings.Contains(info.Name, "test-list-sandbox") {
			found++
			assert.Equal(t, "docker", info.Runtime)
		}
	}

	assert.GreaterOrEqual(t, found, 2, "Expected at least 2 test sandboxes in list")
}

func TestDockerGetLogs(t *testing.T) {
	testutil.SkipIfDockerUnavailable(t)

	adapter, err := NewDockerAdapter()
	require.NoError(t, err)
	defer adapter.Close()

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-logs-sandbox",
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
		Command:   []string{"sh", "-c", "echo 'Hello from sandbox' && sleep 300"},
	}

	cli := testutil.DockerClient(t)
	testutil.PullImage(t, cli, spec.Image)

	sandboxID, err := adapter.Create(ctx, spec)
	require.NoError(t, err)
	defer adapter.Delete(ctx, sandboxID)

	err = adapter.Start(ctx, sandboxID)
	require.NoError(t, err)

	// Wait for log output
	time.Sleep(1 * time.Second)

	// Get logs
	reader, err := adapter.GetLogs(ctx, sandboxID, LogOptions{
		Tail: "100",
	})
	require.NoError(t, err)
	defer reader.Close()

	// Read logs
	logs := make([]byte, 1024)
	n, _ := reader.Read(logs)
	logsStr := string(logs[:n])

	assert.Contains(t, logsStr, "Hello from sandbox")
}

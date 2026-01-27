// Package testutil provides shared testing utilities for the agentic-sandbox project
package testutil

import (
	"context"
	"fmt"
	"io"
	"os"
	"testing"
	"time"

	"github.com/docker/docker/api/types/container"
	"github.com/docker/docker/api/types/image"
	"github.com/docker/docker/client"
	"github.com/stretchr/testify/require"
)

// DockerClient creates a Docker client for tests
func DockerClient(t *testing.T) *client.Client {
	t.Helper()

	cli, err := client.NewClientWithOpts(client.FromEnv, client.WithAPIVersionNegotiation())
	require.NoError(t, err, "Failed to create Docker client")

	t.Cleanup(func() {
		cli.Close()
	})

	return cli
}

// PullImage pulls a Docker image if not already present
func PullImage(t *testing.T, cli *client.Client, imageName string) {
	t.Helper()

	ctx := context.Background()

	// Check if image exists
	_, _, err := cli.ImageInspectWithRaw(ctx, imageName)
	if err == nil {
		return // Image already exists
	}

	t.Logf("Pulling image %s", imageName)
	reader, err := cli.ImagePull(ctx, imageName, image.PullOptions{})
	require.NoError(t, err, "Failed to pull image")
	defer reader.Close()

	// Wait for pull to complete
	_, err = io.Copy(io.Discard, reader)
	require.NoError(t, err, "Failed to read pull output")
}

// CreateTestContainer creates a container for testing with automatic cleanup
func CreateTestContainer(t *testing.T, cli *client.Client, config *container.Config, hostConfig *container.HostConfig) string {
	t.Helper()

	ctx := context.Background()

	// Generate test container name
	name := fmt.Sprintf("test-%s-%d", t.Name(), time.Now().Unix())

	resp, err := cli.ContainerCreate(ctx, config, hostConfig, nil, nil, name)
	require.NoError(t, err, "Failed to create container")

	t.Cleanup(func() {
		cleanupCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()

		// Force remove container
		cli.ContainerRemove(cleanupCtx, resp.ID, container.RemoveOptions{
			Force:         true,
			RemoveVolumes: true,
		})
	})

	return resp.ID
}

// StartContainer starts a container and waits for it to be running
func StartContainer(t *testing.T, cli *client.Client, containerID string) {
	t.Helper()

	ctx := context.Background()

	err := cli.ContainerStart(ctx, containerID, container.StartOptions{})
	require.NoError(t, err, "Failed to start container")

	// Wait for container to be running
	timeout := time.After(10 * time.Second)
	ticker := time.NewTicker(100 * time.Millisecond)
	defer ticker.Stop()

	for {
		select {
		case <-timeout:
			t.Fatal("Container did not start within timeout")
		case <-ticker.C:
			inspect, err := cli.ContainerInspect(ctx, containerID)
			require.NoError(t, err, "Failed to inspect container")

			if inspect.State.Running {
				return
			}

			if inspect.State.Status == "exited" || inspect.State.Status == "dead" {
				t.Fatalf("Container exited unexpectedly: %s", inspect.State.Error)
			}
		}
	}
}

// StopContainer stops a container gracefully
func StopContainer(t *testing.T, cli *client.Client, containerID string) {
	t.Helper()

	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	timeout := 5
	err := cli.ContainerStop(ctx, containerID, container.StopOptions{Timeout: &timeout})
	if err != nil && !client.IsErrNotFound(err) {
		t.Logf("Warning: Failed to stop container: %v", err)
	}
}

// ExecInContainer executes a command in a running container
func ExecInContainer(t *testing.T, cli *client.Client, containerID string, cmd []string) (string, error) {
	t.Helper()

	ctx := context.Background()

	execConfig := container.ExecOptions{
		AttachStdout: true,
		AttachStderr: true,
		Cmd:          cmd,
	}

	execID, err := cli.ContainerExecCreate(ctx, containerID, execConfig)
	if err != nil {
		return "", err
	}

	resp, err := cli.ContainerExecAttach(ctx, execID.ID, container.ExecStartOptions{})
	if err != nil {
		return "", err
	}
	defer resp.Close()

	output, err := io.ReadAll(resp.Reader)
	if err != nil {
		return "", err
	}

	return string(output), nil
}

// WaitForContainerExit waits for a container to exit and returns the exit code
func WaitForContainerExit(t *testing.T, cli *client.Client, containerID string, timeout time.Duration) int64 {
	t.Helper()

	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()

	statusCh, errCh := cli.ContainerWait(ctx, containerID, container.WaitConditionNotRunning)

	select {
	case err := <-errCh:
		require.NoError(t, err, "Error waiting for container")
		return -1
	case status := <-statusCh:
		return status.StatusCode
	case <-ctx.Done():
		t.Fatal("Timeout waiting for container to exit")
		return -1
	}
}

// GetContainerLogs retrieves logs from a container
func GetContainerLogs(t *testing.T, cli *client.Client, containerID string) string {
	t.Helper()

	ctx := context.Background()

	options := container.LogsOptions{
		ShowStdout: true,
		ShowStderr: true,
		Timestamps: false,
	}

	reader, err := cli.ContainerLogs(ctx, containerID, options)
	require.NoError(t, err, "Failed to get container logs")
	defer reader.Close()

	logs, err := io.ReadAll(reader)
	require.NoError(t, err, "Failed to read logs")

	return string(logs)
}

// InspectContainer returns container inspection details
func InspectContainer(t *testing.T, cli *client.Client, containerID string) container.InspectResponse {
	t.Helper()

	ctx := context.Background()

	inspect, err := cli.ContainerInspect(ctx, containerID)
	require.NoError(t, err, "Failed to inspect container")

	return inspect
}

// SkipIfDockerUnavailable skips the test if Docker is not available
func SkipIfDockerUnavailable(t *testing.T) {
	t.Helper()

	cli, err := client.NewClientWithOpts(client.FromEnv, client.WithAPIVersionNegotiation())
	if err != nil {
		t.Skipf("Docker not available: %v", err)
	}
	defer cli.Close()

	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	_, err = cli.Ping(ctx)
	if err != nil {
		t.Skipf("Docker daemon not responding: %v", err)
	}
}

// LoadTestFile loads a test file from testdata directory
func LoadTestFile(t *testing.T, filename string) []byte {
	t.Helper()

	data, err := os.ReadFile(filename)
	require.NoError(t, err, "Failed to read test file %s", filename)

	return data
}

// TempFile creates a temporary file with content for testing
func TempFile(t *testing.T, pattern string, content []byte) string {
	t.Helper()

	f, err := os.CreateTemp("", pattern)
	require.NoError(t, err, "Failed to create temp file")

	_, err = f.Write(content)
	require.NoError(t, err, "Failed to write temp file")

	err = f.Close()
	require.NoError(t, err, "Failed to close temp file")

	t.Cleanup(func() {
		os.Remove(f.Name())
	})

	return f.Name()
}

// AssertContainerRunning asserts that a container is running
func AssertContainerRunning(t *testing.T, cli *client.Client, containerID string) {
	t.Helper()

	inspect := InspectContainer(t, cli, containerID)
	require.True(t, inspect.State.Running, "Container is not running")
}

// AssertContainerStopped asserts that a container is stopped
func AssertContainerStopped(t *testing.T, cli *client.Client, containerID string) {
	t.Helper()

	inspect := InspectContainer(t, cli, containerID)
	require.False(t, inspect.State.Running, "Container is still running")
}

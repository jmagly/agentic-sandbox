package runtime

import (
	"context"
	"fmt"
	"io"
	"strings"

	"github.com/docker/docker/api/types/container"
	"github.com/docker/docker/api/types/filters"
	"github.com/docker/docker/api/types/mount"
	"github.com/docker/docker/api/types/strslice"
	"github.com/docker/docker/client"
)

// DockerAdapter implements RuntimeAdapter for Docker containers
type DockerAdapter struct {
	client *client.Client
}

// NewDockerAdapter creates a new Docker runtime adapter
func NewDockerAdapter() (*DockerAdapter, error) {
	cli, err := client.NewClientWithOpts(client.FromEnv, client.WithAPIVersionNegotiation())
	if err != nil {
		return nil, fmt.Errorf("failed to create Docker client: %w", err)
	}

	return &DockerAdapter{client: cli}, nil
}

// Create creates a new Docker container from the specification
func (d *DockerAdapter) Create(ctx context.Context, spec *SandboxSpec) (string, error) {
	// Convert spec to Docker configuration
	config := &container.Config{
		Image:      spec.Image,
		Hostname:   spec.Network.Hostname,
		Env:        envMapToSlice(spec.Env),
		Labels:     map[string]string{"agentic-sandbox": "true", "sandbox-name": spec.Name},
		WorkingDir: "/workspace",
	}

	if len(spec.Command) > 0 {
		config.Cmd = strslice.StrSlice(spec.Command)
	}

	hostConfig := d.buildHostConfig(spec)

	// Create container
	resp, err := d.client.ContainerCreate(ctx, config, hostConfig, nil, nil, spec.Name)
	if err != nil {
		return "", fmt.Errorf("failed to create container: %w", err)
	}

	return resp.ID, nil
}

// buildHostConfig constructs Docker host configuration from SandboxSpec
func (d *DockerAdapter) buildHostConfig(spec *SandboxSpec) *container.HostConfig {
	hostConfig := &container.HostConfig{
		// Resource limits
		Resources: container.Resources{
			NanoCPUs:   int64(spec.Resources.CPUs * 1e9),
			Memory:     spec.Resources.MemoryMB * 1024 * 1024,
			PidsLimit:  &spec.Resources.PIDsLimit,
		},

		// Network
		NetworkMode: container.NetworkMode(spec.Network.Mode),
		DNS:         spec.Network.DNS,
		DNSSearch:   spec.Network.DNSSearch,
		ExtraHosts:  spec.Network.ExtraHosts,

		// Security
		Privileged:      spec.Security.Privileged,
		ReadonlyRootfs:  spec.Security.ReadOnlyRootFS,
		CapDrop:         strslice.StrSlice(spec.Security.CapDrop),
		CapAdd:          strslice.StrSlice(spec.Security.CapAdd),
		SecurityOpt:     []string{},
	}

	// Security options
	if spec.Security.NoNewPrivileges {
		hostConfig.SecurityOpt = append(hostConfig.SecurityOpt, "no-new-privileges:true")
	}

	if spec.Security.SeccompProfile != "" {
		hostConfig.SecurityOpt = append(hostConfig.SecurityOpt, fmt.Sprintf("seccomp=%s", spec.Security.SeccompProfile))
	}

	if spec.Security.ApparmorProfile != "" {
		hostConfig.SecurityOpt = append(hostConfig.SecurityOpt, fmt.Sprintf("apparmor=%s", spec.Security.ApparmorProfile))
	}

	if spec.Security.SELinuxLabel != "" {
		hostConfig.SecurityOpt = append(hostConfig.SecurityOpt, fmt.Sprintf("label=%s", spec.Security.SELinuxLabel))
	}

	// Mounts
	for _, m := range spec.Mounts {
		hostConfig.Mounts = append(hostConfig.Mounts, mount.Mount{
			Type:     mount.Type(m.Type),
			Source:   m.Source,
			Target:   m.Target,
			ReadOnly: m.ReadOnly,
		})
	}

	// Add writable /tmp if root is read-only
	if spec.Security.ReadOnlyRootFS {
		hostConfig.Tmpfs = map[string]string{
			"/tmp": "noexec,nosuid,size=1g",
		}
	}

	return hostConfig
}

// Start starts a stopped container
func (d *DockerAdapter) Start(ctx context.Context, sandboxID string) error {
	return d.client.ContainerStart(ctx, sandboxID, container.StartOptions{})
}

// Stop stops a running container
func (d *DockerAdapter) Stop(ctx context.Context, sandboxID string) error {
	timeout := 10
	return d.client.ContainerStop(ctx, sandboxID, container.StopOptions{Timeout: &timeout})
}

// Delete removes a container and its resources
func (d *DockerAdapter) Delete(ctx context.Context, sandboxID string) error {
	return d.client.ContainerRemove(ctx, sandboxID, container.RemoveOptions{
		Force:         true,
		RemoveVolumes: true,
	})
}

// Exec executes a command in the container
func (d *DockerAdapter) Exec(ctx context.Context, sandboxID string, cmd []string) (*ExecResult, error) {
	execConfig := container.ExecOptions{
		AttachStdout: true,
		AttachStderr: true,
		Cmd:          cmd,
	}

	execID, err := d.client.ContainerExecCreate(ctx, sandboxID, execConfig)
	if err != nil {
		return nil, fmt.Errorf("failed to create exec: %w", err)
	}

	resp, err := d.client.ContainerExecAttach(ctx, execID.ID, container.ExecStartOptions{})
	if err != nil {
		return nil, fmt.Errorf("failed to attach to exec: %w", err)
	}
	defer resp.Close()

	// Read output
	stdout, err := io.ReadAll(resp.Reader)
	if err != nil {
		return nil, fmt.Errorf("failed to read exec output: %w", err)
	}

	// Get exit code
	inspect, err := d.client.ContainerExecInspect(ctx, execID.ID)
	if err != nil {
		return nil, fmt.Errorf("failed to inspect exec: %w", err)
	}

	return &ExecResult{
		Stdout:   string(stdout),
		Stderr:   "", // Docker combines stdout/stderr in exec attach
		ExitCode: inspect.ExitCode,
	}, nil
}

// GetStatus returns the current status of a container
func (d *DockerAdapter) GetStatus(ctx context.Context, sandboxID string) (*SandboxStatus, error) {
	inspect, err := d.client.ContainerInspect(ctx, sandboxID)
	if err != nil {
		return nil, fmt.Errorf("failed to inspect container: %w", err)
	}

	status := &SandboxStatus{
		ID:      inspect.ID,
		Name:    strings.TrimPrefix(inspect.Name, "/"),
		Runtime: "docker",
		Labels:  inspect.Config.Labels,
	}

	// Map Docker state to sandbox state
	if inspect.State.Running {
		status.State = "running"
	} else if inspect.State.Paused {
		status.State = "paused"
	} else if inspect.State.Restarting {
		status.State = "restarting"
	} else if inspect.State.Dead {
		status.State = "dead"
	} else {
		status.State = "stopped"
	}

	status.StartedAt = inspect.State.StartedAt
	status.FinishedAt = inspect.State.FinishedAt
	status.ExitCode = inspect.State.ExitCode
	status.Error = inspect.State.Error

	// Get resource stats if running
	if inspect.State.Running {
		stats, err := d.client.ContainerStats(ctx, sandboxID, false)
		if err == nil {
			defer stats.Body.Close()
			// Parse stats (simplified for now)
			status.Resources = &ResourceUsage{
				MemoryLimitMB: inspect.HostConfig.Memory / (1024 * 1024),
			}
		}
	}

	return status, nil
}

// List returns all containers managed by this adapter
func (d *DockerAdapter) List(ctx context.Context) ([]SandboxInfo, error) {
	f := filters.NewArgs()
	f.Add("label", "agentic-sandbox=true")
	containers, err := d.client.ContainerList(ctx, container.ListOptions{
		All:     true,
		Filters: f,
	})
	if err != nil {
		return nil, fmt.Errorf("failed to list containers: %w", err)
	}

	infos := make([]SandboxInfo, 0, len(containers))
	for _, c := range containers {
		info := SandboxInfo{
			ID:      c.ID,
			Name:    strings.Join(c.Names, ","),
			Runtime: "docker",
			State:   c.State,
			Image:   c.Image,
			Labels:  c.Labels,
		}
		infos = append(infos, info)
	}

	return infos, nil
}

// GetLogs retrieves logs from a container
func (d *DockerAdapter) GetLogs(ctx context.Context, sandboxID string, opts LogOptions) (io.ReadCloser, error) {
	options := container.LogsOptions{
		ShowStdout: true,
		ShowStderr: true,
		Follow:     opts.Follow,
		Timestamps: opts.Timestamps,
	}

	if opts.Tail != "" {
		options.Tail = opts.Tail
	}

	if opts.Since != "" {
		options.Since = opts.Since
	}

	if opts.Until != "" {
		options.Until = opts.Until
	}

	reader, err := d.client.ContainerLogs(ctx, sandboxID, options)
	if err != nil {
		return nil, fmt.Errorf("failed to get logs: %w", err)
	}

	return reader, nil
}

// Close closes the Docker client
func (d *DockerAdapter) Close() error {
	return d.client.Close()
}

// envMapToSlice converts environment variable map to slice
func envMapToSlice(envMap map[string]string) []string {
	env := make([]string, 0, len(envMap))
	for k, v := range envMap {
		env = append(env, fmt.Sprintf("%s=%s", k, v))
	}
	return env
}

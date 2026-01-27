// Package runtime provides abstractions for different sandbox runtime backends (Docker, QEMU)
package runtime

import (
	"context"
	"io"
)

// RuntimeAdapter defines the interface that all runtime backends must implement
type RuntimeAdapter interface {
	// Create creates a new sandbox instance from the given specification
	Create(ctx context.Context, spec *SandboxSpec) (string, error)

	// Start starts a stopped sandbox
	Start(ctx context.Context, sandboxID string) error

	// Stop stops a running sandbox
	Stop(ctx context.Context, sandboxID string) error

	// Delete removes a sandbox and all its resources
	Delete(ctx context.Context, sandboxID string) error

	// Exec executes a command in the sandbox
	Exec(ctx context.Context, sandboxID string, cmd []string) (*ExecResult, error)

	// GetStatus returns the current status of a sandbox
	GetStatus(ctx context.Context, sandboxID string) (*SandboxStatus, error)

	// List returns all sandboxes managed by this adapter
	List(ctx context.Context) ([]SandboxInfo, error)

	// GetLogs retrieves logs from a sandbox
	GetLogs(ctx context.Context, sandboxID string, opts LogOptions) (io.ReadCloser, error)
}

// SandboxSpec defines the configuration for creating a sandbox
type SandboxSpec struct {
	Name      string            `json:"name" yaml:"name"`
	Image     string            `json:"image" yaml:"image"`
	Runtime   string            `json:"runtime" yaml:"runtime"` // "docker" or "qemu"
	Resources ResourceLimits    `json:"resources" yaml:"resources"`
	Network   NetworkConfig     `json:"network" yaml:"network"`
	Security  SecurityConfig    `json:"security" yaml:"security"`
	Mounts    []MountConfig     `json:"mounts" yaml:"mounts"`
	Env       map[string]string `json:"env" yaml:"env"`
	Command   []string          `json:"command,omitempty" yaml:"command,omitempty"`
}

// ResourceLimits defines resource constraints for a sandbox
type ResourceLimits struct {
	CPUs       float64 `json:"cpus" yaml:"cpus"`             // Number of CPUs (e.g., 2.5)
	MemoryMB   int64   `json:"memory_mb" yaml:"memory_mb"`   // Memory limit in MB
	PIDsLimit  int64   `json:"pids_limit" yaml:"pids_limit"` // Maximum number of processes
	DiskQuotaGB int64  `json:"disk_quota_gb,omitempty" yaml:"disk_quota_gb,omitempty"`
}

// NetworkConfig defines network settings for a sandbox
type NetworkConfig struct {
	Mode       string   `json:"mode" yaml:"mode"`                                 // "none", "bridge", "host"
	Hostname   string   `json:"hostname,omitempty" yaml:"hostname,omitempty"`
	DNS        []string `json:"dns,omitempty" yaml:"dns,omitempty"`
	DNSSearch  []string `json:"dns_search,omitempty" yaml:"dns_search,omitempty"`
	ExtraHosts []string `json:"extra_hosts,omitempty" yaml:"extra_hosts,omitempty"`
}

// SecurityConfig defines security settings for a sandbox
type SecurityConfig struct {
	Privileged       bool     `json:"privileged" yaml:"privileged"`
	ReadOnlyRootFS   bool     `json:"read_only_rootfs" yaml:"read_only_rootfs"`
	NoNewPrivileges  bool     `json:"no_new_privileges" yaml:"no_new_privileges"`
	CapDrop          []string `json:"cap_drop,omitempty" yaml:"cap_drop,omitempty"`
	CapAdd           []string `json:"cap_add,omitempty" yaml:"cap_add,omitempty"`
	SeccompProfile   string   `json:"seccomp_profile,omitempty" yaml:"seccomp_profile,omitempty"`
	ApparmorProfile  string   `json:"apparmor_profile,omitempty" yaml:"apparmor_profile,omitempty"`
	SELinuxLabel     string   `json:"selinux_label,omitempty" yaml:"selinux_label,omitempty"`
}

// MountConfig defines a volume mount
type MountConfig struct {
	Source   string `json:"source" yaml:"source"`
	Target   string `json:"target" yaml:"target"`
	Type     string `json:"type" yaml:"type"`         // "bind", "volume", "tmpfs"
	ReadOnly bool   `json:"read_only" yaml:"read_only"`
}

// SandboxStatus represents the current state of a sandbox
type SandboxStatus struct {
	ID        string            `json:"id"`
	Name      string            `json:"name"`
	State     string            `json:"state"` // "created", "running", "stopped", "error"
	Runtime   string            `json:"runtime"`
	StartedAt string            `json:"started_at,omitempty"`
	FinishedAt string           `json:"finished_at,omitempty"`
	ExitCode  int               `json:"exit_code,omitempty"`
	Error     string            `json:"error,omitempty"`
	Resources *ResourceUsage    `json:"resources,omitempty"`
	Labels    map[string]string `json:"labels,omitempty"`
}

// ResourceUsage represents current resource consumption
type ResourceUsage struct {
	CPUPercent    float64 `json:"cpu_percent"`
	MemoryUsageMB int64   `json:"memory_usage_mb"`
	MemoryLimitMB int64   `json:"memory_limit_mb"`
	PIDs          int     `json:"pids"`
}

// SandboxInfo provides summary information about a sandbox
type SandboxInfo struct {
	ID      string            `json:"id"`
	Name    string            `json:"name"`
	Runtime string            `json:"runtime"`
	State   string            `json:"state"`
	Image   string            `json:"image"`
	Labels  map[string]string `json:"labels,omitempty"`
}

// ExecResult contains the result of executing a command
type ExecResult struct {
	Stdout   string `json:"stdout"`
	Stderr   string `json:"stderr"`
	ExitCode int    `json:"exit_code"`
}

// LogOptions configures log retrieval
type LogOptions struct {
	Follow     bool   `json:"follow"`
	Tail       string `json:"tail"`        // Number of lines (e.g., "100") or "all"
	Since      string `json:"since"`       // Timestamp or relative (e.g., "2h")
	Until      string `json:"until"`
	Timestamps bool   `json:"timestamps"`
}

// DefaultResourceLimits returns safe default resource limits
func DefaultResourceLimits() ResourceLimits {
	return ResourceLimits{
		CPUs:      4.0,
		MemoryMB:  8192, // 8GB
		PIDsLimit: 1024,
	}
}

// DefaultSecurityConfig returns hardened default security settings
func DefaultSecurityConfig() SecurityConfig {
	return SecurityConfig{
		Privileged:      false,
		ReadOnlyRootFS:  true,
		NoNewPrivileges: true,
		CapDrop:         []string{"ALL"},
		CapAdd:          []string{}, // No capabilities by default
	}
}

// DefaultNetworkConfig returns isolated network configuration
func DefaultNetworkConfig() NetworkConfig {
	return NetworkConfig{
		Mode: "none", // No network by default
	}
}

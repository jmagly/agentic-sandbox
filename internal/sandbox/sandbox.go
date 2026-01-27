package sandbox

import (
	"time"
)

// SandboxState represents the current state of a sandbox
type SandboxState string

const (
	StateCreated SandboxState = "created"
	StateRunning SandboxState = "running"
	StateStopped SandboxState = "stopped"
	StateDeleted SandboxState = "deleted"
	StateError   SandboxState = "error"
)

// Resources defines resource limits for a sandbox
type Resources struct {
	CPU        string `json:"cpu"`         // CPU count or quota (e.g., "4" or "2.5")
	Memory     string `json:"memory"`      // Memory limit (e.g., "8G", "512M")
	PidsLimit  int    `json:"pids_limit"`  // Maximum number of processes
	DiskQuota  string `json:"disk_quota"`  // Disk quota (e.g., "50G")
}

// Mount represents a volume mount
type Mount struct {
	Source      string `json:"source"`
	Destination string `json:"destination"`
	ReadOnly    bool   `json:"read_only"`
}

// NetworkMode defines network configuration
type NetworkMode string

const (
	NetworkIsolated NetworkMode = "isolated" // No network access
	NetworkGateway  NetworkMode = "gateway"  // Access via gateway only
	NetworkHost     NetworkMode = "host"     // Full network access (not recommended)
)

// Sandbox represents an isolated agent environment
type Sandbox struct {
	ID          string            `json:"id"`
	Name        string            `json:"name"`
	Runtime     string            `json:"runtime"`      // "docker" or "qemu"
	Image       string            `json:"image"`
	State       SandboxState      `json:"state"`
	Resources   Resources         `json:"resources"`
	Network     NetworkMode       `json:"network"`
	GatewayURL  string            `json:"gateway_url,omitempty"`
	Mounts      []Mount           `json:"mounts,omitempty"`
	Environment map[string]string `json:"environment,omitempty"`
	CreatedAt   time.Time         `json:"created_at"`
	StartedAt   *time.Time        `json:"started_at,omitempty"`
	StoppedAt   *time.Time        `json:"stopped_at,omitempty"`
	ErrorMsg    string            `json:"error_msg,omitempty"`
}

// SandboxSpec is used to create a new sandbox
type SandboxSpec struct {
	Name        string            `json:"name"`
	Runtime     string            `json:"runtime"`      // "docker" or "qemu"
	Image       string            `json:"image"`
	Resources   Resources         `json:"resources"`
	Network     NetworkMode       `json:"network"`
	GatewayURL  string            `json:"gateway_url,omitempty"`
	Mounts      []Mount           `json:"mounts,omitempty"`
	Environment map[string]string `json:"environment,omitempty"`
	AutoStart   bool              `json:"auto_start"`   // Start immediately after creation
}

// DefaultResources returns sensible default resource limits
func DefaultResources() Resources {
	return Resources{
		CPU:       "4",
		Memory:    "8G",
		PidsLimit: 1024,
		DiskQuota: "50G",
	}
}

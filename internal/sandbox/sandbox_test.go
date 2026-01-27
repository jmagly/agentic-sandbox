package sandbox

import (
	"testing"
	"time"
)

func TestDefaultResources(t *testing.T) {
	res := DefaultResources()

	if res.CPU != "4" {
		t.Errorf("expected CPU to be '4', got '%s'", res.CPU)
	}
	if res.Memory != "8G" {
		t.Errorf("expected Memory to be '8G', got '%s'", res.Memory)
	}
	if res.PidsLimit != 1024 {
		t.Errorf("expected PidsLimit to be 1024, got %d", res.PidsLimit)
	}
	if res.DiskQuota != "50G" {
		t.Errorf("expected DiskQuota to be '50G', got '%s'", res.DiskQuota)
	}
}

func TestSandboxCreation(t *testing.T) {
	now := time.Now()
	sb := &Sandbox{
		ID:      "test-123",
		Name:    "test-sandbox",
		Runtime: "docker",
		Image:   "agent-claude",
		State:   StateCreated,
		Resources: Resources{
			CPU:       "2",
			Memory:    "4G",
			PidsLimit: 512,
		},
		Network:   NetworkIsolated,
		CreatedAt: now,
	}

	if sb.ID != "test-123" {
		t.Errorf("expected ID 'test-123', got '%s'", sb.ID)
	}
	if sb.State != StateCreated {
		t.Errorf("expected state 'created', got '%s'", sb.State)
	}
	if sb.StartedAt != nil {
		t.Errorf("expected StartedAt to be nil for created sandbox")
	}
}

func TestSandboxSpec(t *testing.T) {
	spec := &SandboxSpec{
		Name:    "my-agent",
		Runtime: "docker",
		Image:   "agent-claude:latest",
		Resources: Resources{
			CPU:       "4",
			Memory:    "8G",
			PidsLimit: 1024,
		},
		Network:   NetworkGateway,
		GatewayURL: "http://gateway:8080",
		Environment: map[string]string{
			"AGENT_TASK": "refactor auth module",
		},
		AutoStart: true,
	}

	if spec.Runtime != "docker" {
		t.Errorf("expected runtime 'docker', got '%s'", spec.Runtime)
	}
	if spec.AutoStart != true {
		t.Errorf("expected AutoStart to be true")
	}
	if spec.Environment["AGENT_TASK"] != "refactor auth module" {
		t.Errorf("expected AGENT_TASK environment variable")
	}
}

func TestNetworkModes(t *testing.T) {
	tests := []struct {
		name string
		mode NetworkMode
	}{
		{"isolated", NetworkIsolated},
		{"gateway", NetworkGateway},
		{"host", NetworkHost},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			sb := &Sandbox{
				Network: tt.mode,
			}
			if sb.Network != tt.mode {
				t.Errorf("expected network mode '%s', got '%s'", tt.mode, sb.Network)
			}
		})
	}
}

func TestSandboxStates(t *testing.T) {
	states := []SandboxState{
		StateCreated,
		StateRunning,
		StateStopped,
		StateDeleted,
		StateError,
	}

	for _, state := range states {
		sb := &Sandbox{State: state}
		if sb.State != state {
			t.Errorf("expected state '%s', got '%s'", state, sb.State)
		}
	}
}

package sandbox

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/rs/zerolog/log"
)

// Manager handles sandbox lifecycle operations
type Manager struct {
	mu        sync.RWMutex
	sandboxes map[string]*Sandbox
	// runtimeAdapters will be added when runtime package is imported
}

// NewManager creates a new sandbox manager
func NewManager() *Manager {
	return &Manager{
		sandboxes: make(map[string]*Sandbox),
	}
}

// Create creates a new sandbox
func (m *Manager) Create(ctx context.Context, spec *SandboxSpec) (*Sandbox, error) {
	m.mu.Lock()
	defer m.mu.Unlock()

	// Validate spec
	if spec.Name == "" {
		return nil, fmt.Errorf("sandbox name is required")
	}
	if spec.Runtime == "" {
		return nil, fmt.Errorf("runtime is required")
	}
	if spec.Image == "" {
		return nil, fmt.Errorf("image is required")
	}

	// Generate ID
	id := generateSandboxID(spec.Name)

	// Check for duplicate
	if _, exists := m.sandboxes[id]; exists {
		return nil, fmt.Errorf("sandbox with ID %s already exists", id)
	}

	// Create sandbox object
	sb := &Sandbox{
		ID:          id,
		Name:        spec.Name,
		Runtime:     spec.Runtime,
		Image:       spec.Image,
		State:       StateCreated,
		Resources:   spec.Resources,
		Network:     spec.Network,
		GatewayURL:  spec.GatewayURL,
		Mounts:      spec.Mounts,
		Environment: spec.Environment,
		CreatedAt:   time.Now(),
	}

	// Store sandbox
	m.sandboxes[id] = sb

	log.Info().
		Str("id", id).
		Str("name", spec.Name).
		Str("runtime", spec.Runtime).
		Msg("sandbox created")

	// TODO: Call runtime adapter to create actual container/VM

	// Auto-start if requested (must release lock first to avoid deadlock)
	if spec.AutoStart {
		m.mu.Unlock()
		err := m.Start(ctx, id)
		m.mu.Lock()
		if err != nil {
			return sb, fmt.Errorf("failed to auto-start sandbox: %w", err)
		}
	}

	return sb, nil
}

// Get retrieves a sandbox by ID
func (m *Manager) Get(ctx context.Context, id string) (*Sandbox, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	sb, exists := m.sandboxes[id]
	if !exists {
		return nil, fmt.Errorf("sandbox %s not found", id)
	}

	return sb, nil
}

// List returns all sandboxes
func (m *Manager) List(ctx context.Context) ([]*Sandbox, error) {
	m.mu.RLock()
	defer m.mu.RUnlock()

	sandboxes := make([]*Sandbox, 0, len(m.sandboxes))
	for _, sb := range m.sandboxes {
		sandboxes = append(sandboxes, sb)
	}

	return sandboxes, nil
}

// Start starts a sandbox
func (m *Manager) Start(ctx context.Context, id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	sb, exists := m.sandboxes[id]
	if !exists {
		return fmt.Errorf("sandbox %s not found", id)
	}

	if sb.State == StateRunning {
		return fmt.Errorf("sandbox %s is already running", id)
	}

	// TODO: Call runtime adapter to start container/VM

	now := time.Now()
	sb.State = StateRunning
	sb.StartedAt = &now

	log.Info().
		Str("id", id).
		Str("name", sb.Name).
		Msg("sandbox started")

	return nil
}

// Stop stops a sandbox
func (m *Manager) Stop(ctx context.Context, id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	sb, exists := m.sandboxes[id]
	if !exists {
		return fmt.Errorf("sandbox %s not found", id)
	}

	if sb.State != StateRunning {
		return fmt.Errorf("sandbox %s is not running", id)
	}

	// TODO: Call runtime adapter to stop container/VM

	now := time.Now()
	sb.State = StateStopped
	sb.StoppedAt = &now

	log.Info().
		Str("id", id).
		Str("name", sb.Name).
		Msg("sandbox stopped")

	return nil
}

// Delete deletes a sandbox
func (m *Manager) Delete(ctx context.Context, id string) error {
	m.mu.Lock()
	defer m.mu.Unlock()

	sb, exists := m.sandboxes[id]
	if !exists {
		return fmt.Errorf("sandbox %s not found", id)
	}

	// Stop if running
	if sb.State == StateRunning {
		// TODO: Call runtime adapter to stop
		now := time.Now()
		sb.StoppedAt = &now
	}

	// TODO: Call runtime adapter to delete container/VM

	sb.State = StateDeleted
	delete(m.sandboxes, id)

	log.Info().
		Str("id", id).
		Str("name", sb.Name).
		Msg("sandbox deleted")

	return nil
}

var idCounter int64

// generateSandboxID creates a unique sandbox ID
func generateSandboxID(name string) string {
	// Simple implementation: name + timestamp + counter
	idCounter++
	return fmt.Sprintf("%s-%d-%d", name, time.Now().UnixNano(), idCounter)
}

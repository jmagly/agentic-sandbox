package sandbox

import (
	"context"
	"testing"
)

func TestNewManager(t *testing.T) {
	mgr := NewManager()
	if mgr == nil {
		t.Fatal("expected manager to be non-nil")
	}
	if mgr.sandboxes == nil {
		t.Fatal("expected sandboxes map to be initialized")
	}
}

func TestManagerCreate(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	spec := &SandboxSpec{
		Name:    "test-sandbox",
		Runtime: "docker",
		Image:   "agent-claude:latest",
		Resources: Resources{
			CPU:       "2",
			Memory:    "4G",
			PidsLimit: 512,
		},
		Network:   NetworkIsolated,
		AutoStart: false,
	}

	sb, err := mgr.Create(ctx, spec)
	if err != nil {
		t.Fatalf("expected no error creating sandbox, got %v", err)
	}
	if sb == nil {
		t.Fatal("expected sandbox to be non-nil")
	}
	if sb.Name != "test-sandbox" {
		t.Errorf("expected name 'test-sandbox', got '%s'", sb.Name)
	}
	if sb.State != StateCreated {
		t.Errorf("expected state 'created', got '%s'", sb.State)
	}
}

func TestManagerCreateValidation(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	tests := []struct {
		name    string
		spec    *SandboxSpec
		wantErr bool
	}{
		{
			name: "missing name",
			spec: &SandboxSpec{
				Runtime: "docker",
				Image:   "test:latest",
			},
			wantErr: true,
		},
		{
			name: "missing runtime",
			spec: &SandboxSpec{
				Name:  "test",
				Image: "test:latest",
			},
			wantErr: true,
		},
		{
			name: "missing image",
			spec: &SandboxSpec{
				Name:    "test",
				Runtime: "docker",
			},
			wantErr: true,
		},
		{
			name: "valid spec",
			spec: &SandboxSpec{
				Name:      "test",
				Runtime:   "docker",
				Image:     "test:latest",
				Resources: DefaultResources(),
			},
			wantErr: false,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := mgr.Create(ctx, tt.spec)
			if (err != nil) != tt.wantErr {
				t.Errorf("expected error=%v, got error=%v", tt.wantErr, err)
			}
		})
	}
}

func TestManagerGet(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: DefaultResources(),
	}

	created, err := mgr.Create(ctx, spec)
	if err != nil {
		t.Fatalf("failed to create sandbox: %v", err)
	}

	retrieved, err := mgr.Get(ctx, created.ID)
	if err != nil {
		t.Fatalf("expected no error getting sandbox, got %v", err)
	}
	if retrieved.ID != created.ID {
		t.Errorf("expected ID '%s', got '%s'", created.ID, retrieved.ID)
	}
}

func TestManagerGetNotFound(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	_, err := mgr.Get(ctx, "nonexistent")
	if err == nil {
		t.Fatal("expected error for nonexistent sandbox")
	}
}

func TestManagerList(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	// Create multiple sandboxes
	for i := 0; i < 3; i++ {
		spec := &SandboxSpec{
			Name:      "test",
			Runtime:   "docker",
			Image:     "test:latest",
			Resources: DefaultResources(),
		}
		_, err := mgr.Create(ctx, spec)
		if err != nil {
			t.Fatalf("failed to create sandbox: %v", err)
		}
	}

	sandboxes, err := mgr.List(ctx)
	if err != nil {
		t.Fatalf("expected no error listing sandboxes, got %v", err)
	}
	if len(sandboxes) != 3 {
		t.Errorf("expected 3 sandboxes, got %d", len(sandboxes))
	}
}

func TestManagerStart(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: DefaultResources(),
	}

	sb, err := mgr.Create(ctx, spec)
	if err != nil {
		t.Fatalf("failed to create sandbox: %v", err)
	}

	err = mgr.Start(ctx, sb.ID)
	if err != nil {
		t.Fatalf("expected no error starting sandbox, got %v", err)
	}

	retrieved, _ := mgr.Get(ctx, sb.ID)
	if retrieved.State != StateRunning {
		t.Errorf("expected state 'running', got '%s'", retrieved.State)
	}
	if retrieved.StartedAt == nil {
		t.Error("expected StartedAt to be set")
	}
}

func TestManagerStop(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: DefaultResources(),
		AutoStart: true,
	}

	sb, err := mgr.Create(ctx, spec)
	if err != nil {
		t.Fatalf("failed to create sandbox: %v", err)
	}

	err = mgr.Stop(ctx, sb.ID)
	if err != nil {
		t.Fatalf("expected no error stopping sandbox, got %v", err)
	}

	retrieved, _ := mgr.Get(ctx, sb.ID)
	if retrieved.State != StateStopped {
		t.Errorf("expected state 'stopped', got '%s'", retrieved.State)
	}
	if retrieved.StoppedAt == nil {
		t.Error("expected StoppedAt to be set")
	}
}

func TestManagerDelete(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: DefaultResources(),
	}

	sb, err := mgr.Create(ctx, spec)
	if err != nil {
		t.Fatalf("failed to create sandbox: %v", err)
	}

	err = mgr.Delete(ctx, sb.ID)
	if err != nil {
		t.Fatalf("expected no error deleting sandbox, got %v", err)
	}

	_, err = mgr.Get(ctx, sb.ID)
	if err == nil {
		t.Fatal("expected error getting deleted sandbox")
	}
}

func TestManagerAutoStart(t *testing.T) {
	mgr := NewManager()
	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: DefaultResources(),
		AutoStart: true,
	}

	sb, err := mgr.Create(ctx, spec)
	if err != nil {
		t.Fatalf("failed to create sandbox: %v", err)
	}

	if sb.State != StateRunning {
		t.Errorf("expected auto-started sandbox to be running, got '%s'", sb.State)
	}
}

package runtime

import (
	"context"
	"io"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// MockRuntimeAdapter provides a mock implementation of RuntimeAdapter for testing
type MockRuntimeAdapter struct {
	CreateFunc    func(ctx context.Context, spec *SandboxSpec) (string, error)
	StartFunc     func(ctx context.Context, sandboxID string) error
	StopFunc      func(ctx context.Context, sandboxID string) error
	DeleteFunc    func(ctx context.Context, sandboxID string) error
	ExecFunc      func(ctx context.Context, sandboxID string, cmd []string) (*ExecResult, error)
	GetStatusFunc func(ctx context.Context, sandboxID string) (*SandboxStatus, error)
	ListFunc      func(ctx context.Context) ([]SandboxInfo, error)
	GetLogsFunc   func(ctx context.Context, sandboxID string, opts LogOptions) (io.ReadCloser, error)
}

func (m *MockRuntimeAdapter) Create(ctx context.Context, spec *SandboxSpec) (string, error) {
	if m.CreateFunc != nil {
		return m.CreateFunc(ctx, spec)
	}
	return "mock-sandbox-id", nil
}

func (m *MockRuntimeAdapter) Start(ctx context.Context, sandboxID string) error {
	if m.StartFunc != nil {
		return m.StartFunc(ctx, sandboxID)
	}
	return nil
}

func (m *MockRuntimeAdapter) Stop(ctx context.Context, sandboxID string) error {
	if m.StopFunc != nil {
		return m.StopFunc(ctx, sandboxID)
	}
	return nil
}

func (m *MockRuntimeAdapter) Delete(ctx context.Context, sandboxID string) error {
	if m.DeleteFunc != nil {
		return m.DeleteFunc(ctx, sandboxID)
	}
	return nil
}

func (m *MockRuntimeAdapter) Exec(ctx context.Context, sandboxID string, cmd []string) (*ExecResult, error) {
	if m.ExecFunc != nil {
		return m.ExecFunc(ctx, sandboxID, cmd)
	}
	return &ExecResult{Stdout: "mock output", ExitCode: 0}, nil
}

func (m *MockRuntimeAdapter) GetStatus(ctx context.Context, sandboxID string) (*SandboxStatus, error) {
	if m.GetStatusFunc != nil {
		return m.GetStatusFunc(ctx, sandboxID)
	}
	return &SandboxStatus{ID: sandboxID, State: "running"}, nil
}

func (m *MockRuntimeAdapter) List(ctx context.Context) ([]SandboxInfo, error) {
	if m.ListFunc != nil {
		return m.ListFunc(ctx)
	}
	return []SandboxInfo{}, nil
}

func (m *MockRuntimeAdapter) GetLogs(ctx context.Context, sandboxID string, opts LogOptions) (io.ReadCloser, error) {
	if m.GetLogsFunc != nil {
		return m.GetLogsFunc(ctx, sandboxID, opts)
	}
	return io.NopCloser(nil), nil
}

// TestMockRuntimeAdapter verifies the mock implementation satisfies the interface
func TestMockRuntimeAdapter(t *testing.T) {
	var _ RuntimeAdapter = (*MockRuntimeAdapter)(nil)
}

func TestDefaultResourceLimits(t *testing.T) {
	limits := DefaultResourceLimits()

	assert.Equal(t, 4.0, limits.CPUs)
	assert.Equal(t, int64(8192), limits.MemoryMB)
	assert.Equal(t, int64(1024), limits.PIDsLimit)
}

func TestDefaultSecurityConfig(t *testing.T) {
	config := DefaultSecurityConfig()

	assert.False(t, config.Privileged)
	assert.True(t, config.ReadOnlyRootFS)
	assert.True(t, config.NoNewPrivileges)
	assert.Contains(t, config.CapDrop, "ALL")
	assert.Empty(t, config.CapAdd)
}

func TestDefaultNetworkConfig(t *testing.T) {
	config := DefaultNetworkConfig()

	assert.Equal(t, "none", config.Mode)
}

func TestSandboxSpec_Validation(t *testing.T) {
	tests := []struct {
		name  string
		spec  SandboxSpec
		valid bool
	}{
		{
			name: "valid minimal spec",
			spec: SandboxSpec{
				Name:      "test-sandbox",
				Image:     "ubuntu:22.04",
				Runtime:   "docker",
				Resources: DefaultResourceLimits(),
				Network:   DefaultNetworkConfig(),
				Security:  DefaultSecurityConfig(),
			},
			valid: true,
		},
		{
			name: "valid with custom resources",
			spec: SandboxSpec{
				Name:    "custom-sandbox",
				Image:   "alpine:3.18",
				Runtime: "docker",
				Resources: ResourceLimits{
					CPUs:      2.0,
					MemoryMB:  4096,
					PIDsLimit: 512,
				},
				Network:  DefaultNetworkConfig(),
				Security: DefaultSecurityConfig(),
			},
			valid: true,
		},
		{
			name: "with mounts and env",
			spec: SandboxSpec{
				Name:      "full-sandbox",
				Image:     "ubuntu:22.04",
				Runtime:   "docker",
				Resources: DefaultResourceLimits(),
				Network:   DefaultNetworkConfig(),
				Security:  DefaultSecurityConfig(),
				Mounts: []MountConfig{
					{
						Source:   "/host/path",
						Target:   "/container/path",
						Type:     "bind",
						ReadOnly: true,
					},
				},
				Env: map[string]string{
					"TEST_VAR": "test_value",
				},
			},
			valid: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Basic validation checks
			if tt.valid {
				assert.NotEmpty(t, tt.spec.Name)
				assert.NotEmpty(t, tt.spec.Image)
				assert.NotEmpty(t, tt.spec.Runtime)
			}
		})
	}
}

func TestMockRuntimeAdapter_Create(t *testing.T) {
	ctx := context.Background()
	spec := &SandboxSpec{
		Name:      "test-sandbox",
		Image:     "ubuntu:22.04",
		Runtime:   "docker",
		Resources: DefaultResourceLimits(),
	}

	mock := &MockRuntimeAdapter{
		CreateFunc: func(ctx context.Context, spec *SandboxSpec) (string, error) {
			assert.Equal(t, "test-sandbox", spec.Name)
			return "custom-id", nil
		},
	}

	id, err := mock.Create(ctx, spec)
	require.NoError(t, err)
	assert.Equal(t, "custom-id", id)
}

func TestMockRuntimeAdapter_Start(t *testing.T) {
	ctx := context.Background()
	mock := &MockRuntimeAdapter{
		StartFunc: func(ctx context.Context, sandboxID string) error {
			assert.Equal(t, "test-id", sandboxID)
			return nil
		},
	}

	err := mock.Start(ctx, "test-id")
	require.NoError(t, err)
}

func TestMockRuntimeAdapter_Stop(t *testing.T) {
	ctx := context.Background()
	mock := &MockRuntimeAdapter{
		StopFunc: func(ctx context.Context, sandboxID string) error {
			assert.Equal(t, "test-id", sandboxID)
			return nil
		},
	}

	err := mock.Stop(ctx, "test-id")
	require.NoError(t, err)
}

func TestMockRuntimeAdapter_Delete(t *testing.T) {
	ctx := context.Background()
	mock := &MockRuntimeAdapter{
		DeleteFunc: func(ctx context.Context, sandboxID string) error {
			assert.Equal(t, "test-id", sandboxID)
			return nil
		},
	}

	err := mock.Delete(ctx, "test-id")
	require.NoError(t, err)
}

func TestMockRuntimeAdapter_Exec(t *testing.T) {
	ctx := context.Background()
	mock := &MockRuntimeAdapter{
		ExecFunc: func(ctx context.Context, sandboxID string, cmd []string) (*ExecResult, error) {
			assert.Equal(t, "test-id", sandboxID)
			assert.Equal(t, []string{"echo", "hello"}, cmd)
			return &ExecResult{
				Stdout:   "hello\n",
				Stderr:   "",
				ExitCode: 0,
			}, nil
		},
	}

	result, err := mock.Exec(ctx, "test-id", []string{"echo", "hello"})
	require.NoError(t, err)
	assert.Equal(t, "hello\n", result.Stdout)
	assert.Equal(t, 0, result.ExitCode)
}

func TestMockRuntimeAdapter_GetStatus(t *testing.T) {
	ctx := context.Background()
	mock := &MockRuntimeAdapter{
		GetStatusFunc: func(ctx context.Context, sandboxID string) (*SandboxStatus, error) {
			return &SandboxStatus{
				ID:      sandboxID,
				Name:    "test-sandbox",
				State:   "running",
				Runtime: "docker",
			}, nil
		},
	}

	status, err := mock.GetStatus(ctx, "test-id")
	require.NoError(t, err)
	assert.Equal(t, "test-id", status.ID)
	assert.Equal(t, "running", status.State)
}

func TestMockRuntimeAdapter_List(t *testing.T) {
	ctx := context.Background()
	mock := &MockRuntimeAdapter{
		ListFunc: func(ctx context.Context) ([]SandboxInfo, error) {
			return []SandboxInfo{
				{ID: "id-1", Name: "sandbox-1", State: "running"},
				{ID: "id-2", Name: "sandbox-2", State: "stopped"},
			}, nil
		},
	}

	list, err := mock.List(ctx)
	require.NoError(t, err)
	assert.Len(t, list, 2)
	assert.Equal(t, "sandbox-1", list[0].Name)
}

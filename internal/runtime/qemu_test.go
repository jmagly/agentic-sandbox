//go:build integration
// +build integration

package runtime

import (
	"context"
	"os/exec"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func skipIfLibvirtUnavailable(t *testing.T) {
	t.Helper()

	_, err := exec.LookPath("virsh")
	if err != nil {
		t.Skip("virsh not available, skipping QEMU tests")
	}

	// Check if we can connect to libvirt
	cmd := exec.Command("virsh", "version")
	if err := cmd.Run(); err != nil {
		t.Skip("libvirt daemon not accessible, skipping QEMU tests")
	}
}

func TestQEMUCreate(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	spec := &SandboxSpec{
		Name:      "test-qemu-sandbox",
		Image:     "ubuntu-22.04",
		Runtime:   "qemu",
		Resources: DefaultResourceLimits(),
		Network:   DefaultNetworkConfig(),
		Security:  DefaultSecurityConfig(),
	}

	// This will fail with "not fully implemented" for now
	_, err = adapter.Create(ctx, spec)
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "not fully implemented")
}

func TestQEMUStartStop(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	// Check if test VM exists
	cmd := exec.CommandContext(ctx, "virsh", "list", "--all", "--name")
	output, err := cmd.CombinedOutput()
	require.NoError(t, err)

	// For this test to work, you'd need a pre-existing test VM
	// Skip if no test VM is available
	t.Skip("Requires pre-existing test VM for lifecycle testing")
}

func TestQEMUExec(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	// This will fail with "not fully implemented" for now
	_, err = adapter.Exec(ctx, "test-vm", []string{"echo", "hello"})
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "not fully implemented")
}

func TestQEMUGetStatus(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	// List all VMs to find one for testing
	list, err := adapter.List(ctx)
	require.NoError(t, err)

	if len(list) == 0 {
		t.Skip("No VMs available for testing")
	}

	// Get status of first VM
	vmName := list[0].ID
	status, err := adapter.GetStatus(ctx, vmName)
	require.NoError(t, err)
	assert.Equal(t, vmName, status.ID)
	assert.Equal(t, "qemu", status.Runtime)
	assert.NotEmpty(t, status.State)
}

func TestQEMUList(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	list, err := adapter.List(ctx)
	require.NoError(t, err)

	// Should return a list (may be empty)
	assert.NotNil(t, list)

	// If there are VMs, verify structure
	for _, info := range list {
		assert.NotEmpty(t, info.ID)
		assert.NotEmpty(t, info.Name)
		assert.Equal(t, "qemu", info.Runtime)
		assert.NotEmpty(t, info.State)
	}
}

func TestQEMUDelete(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	// Attempting to delete non-existent VM should fail gracefully
	err = adapter.Delete(ctx, "non-existent-vm")
	assert.Error(t, err)
}

func TestQEMUGetLogs(t *testing.T) {
	skipIfLibvirtUnavailable(t)

	adapter, err := NewQEMUAdapter()
	require.NoError(t, err)

	ctx := context.Background()

	// This will fail with "not fully implemented" for now
	_, err = adapter.GetLogs(ctx, "test-vm", LogOptions{})
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "not fully implemented")
}

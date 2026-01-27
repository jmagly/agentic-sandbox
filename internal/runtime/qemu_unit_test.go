package runtime

import (
	"context"
	"encoding/xml"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestQEMUAdapterWithPaths(t *testing.T) {
	tempDir := t.TempDir()
	imagesDir := filepath.Join(tempDir, "images")
	logsDir := filepath.Join(tempDir, "logs")

	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", imagesDir, logsDir)

	assert.NotNil(t, adapter)
	assert.Equal(t, "/usr/bin/virsh", adapter.virshPath)
	assert.Equal(t, imagesDir, adapter.imagesDir)
	assert.Equal(t, logsDir, adapter.consoleLogsDir)
}

func TestBuildDomainXML_Minimal(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp/images", "/tmp/logs")

	spec := &SandboxSpec{
		Name:    "test-sandbox",
		Image:   "ubuntu-22.04",
		Runtime: "qemu",
		Resources: ResourceLimits{
			CPUs:     2,
			MemoryMB: 4096,
		},
		Network: NetworkConfig{
			Mode: "none",
		},
	}

	domain := adapter.buildDomainXML("test-vm-123", spec, "/tmp/disk.qcow2", "/tmp/console.log")

	assert.Equal(t, "kvm", domain.Type)
	assert.Equal(t, "test-vm-123", domain.Name)
	assert.Equal(t, int64(4096), domain.Memory.Value)
	assert.Equal(t, "MiB", domain.Memory.Unit)
	assert.Equal(t, 2, domain.VCPU.Value)
	assert.Equal(t, "x86_64", domain.OS.Type.Arch)
	assert.Equal(t, "hvm", domain.OS.Type.Value)
	assert.Equal(t, "host-passthrough", domain.CPU.Mode)
	assert.Len(t, domain.Devices.Disks, 1)
	assert.Equal(t, "/tmp/disk.qcow2", domain.Devices.Disks[0].Source.File)
	assert.Len(t, domain.Devices.Interfaces, 0) // No network for "none" mode
}

func TestBuildDomainXML_WithNetwork(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp/images", "/tmp/logs")

	spec := &SandboxSpec{
		Name:    "test-sandbox",
		Image:   "ubuntu-22.04",
		Runtime: "qemu",
		Resources: ResourceLimits{
			CPUs:     1,
			MemoryMB: 2048,
		},
		Network: NetworkConfig{
			Mode: "bridge",
		},
	}

	domain := adapter.buildDomainXML("test-vm-123", spec, "/tmp/disk.qcow2", "/tmp/console.log")

	assert.Len(t, domain.Devices.Interfaces, 1)
	assert.Equal(t, "network", domain.Devices.Interfaces[0].Type)
	assert.Equal(t, "default", domain.Devices.Interfaces[0].Source.Network)
	assert.Equal(t, "virtio", domain.Devices.Interfaces[0].Model.Type)
}

func TestBuildDomainXML_WithMounts(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp/images", "/tmp/logs")

	spec := &SandboxSpec{
		Name:    "test-sandbox",
		Image:   "ubuntu-22.04",
		Runtime: "qemu",
		Resources: ResourceLimits{
			CPUs:     1,
			MemoryMB: 2048,
		},
		Network: NetworkConfig{
			Mode: "none",
		},
		Mounts: []MountConfig{
			{
				Source:   "/host/workspace",
				Target:   "/workspace",
				Type:     "bind",
				ReadOnly: false,
			},
			{
				Source:   "/host/data",
				Target:   "/data",
				Type:     "bind",
				ReadOnly: true,
			},
		},
	}

	domain := adapter.buildDomainXML("test-vm-123", spec, "/tmp/disk.qcow2", "/tmp/console.log")

	assert.Len(t, domain.Devices.Filesystems, 2)
	assert.Equal(t, "/host/workspace", domain.Devices.Filesystems[0].Source.Dir)
	assert.Equal(t, "/workspace", domain.Devices.Filesystems[0].Target.Dir)
	assert.Nil(t, domain.Devices.Filesystems[0].Readonly)
	assert.Equal(t, "/host/data", domain.Devices.Filesystems[1].Source.Dir)
	assert.NotNil(t, domain.Devices.Filesystems[1].Readonly)
}

func TestBuildDomainXML_MinimalCPU(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp/images", "/tmp/logs")

	spec := &SandboxSpec{
		Name:    "test-sandbox",
		Image:   "ubuntu-22.04",
		Runtime: "qemu",
		Resources: ResourceLimits{
			CPUs:     0.5, // Less than 1
			MemoryMB: 1024,
		},
		Network: NetworkConfig{
			Mode: "none",
		},
	}

	domain := adapter.buildDomainXML("test-vm-123", spec, "/tmp/disk.qcow2", "/tmp/console.log")

	// Should be at least 1 vCPU
	assert.Equal(t, 1, domain.VCPU.Value)
}

func TestLibvirtDomainXML_Marshaling(t *testing.T) {
	domain := LibvirtDomain{
		Type: "kvm",
		Name: "test-vm",
		Memory: LibvirtMemory{
			Unit:  "MiB",
			Value: 2048,
		},
		VCPU: LibvirtVCPU{
			Placement: "static",
			Value:     2,
		},
		OS: LibvirtOS{
			Type: LibvirtOSType{
				Arch:    "x86_64",
				Machine: "q35",
				Value:   "hvm",
			},
			Boot: LibvirtBoot{Dev: "hd"},
		},
		Features: LibvirtFeatures{},
		CPU:      LibvirtCPU{Mode: "host-passthrough"},
		Devices: LibvirtDevices{
			Emulator: "/usr/bin/qemu-system-x86_64",
			Disks: []LibvirtDisk{
				{
					Type:   "file",
					Device: "disk",
					Driver: LibvirtDiskDriver{Name: "qemu", Type: "qcow2"},
					Source: LibvirtDiskSource{File: "/var/lib/libvirt/images/test.qcow2"},
					Target: LibvirtDiskTarget{Dev: "vda", Bus: "virtio"},
				},
			},
		},
		OnPoweroff: "destroy",
		OnReboot:   "restart",
		OnCrash:    "destroy",
	}

	xmlData, err := xml.MarshalIndent(domain, "", "  ")
	require.NoError(t, err)

	xmlStr := string(xmlData)
	assert.Contains(t, xmlStr, `type="kvm"`)
	assert.Contains(t, xmlStr, "<name>test-vm</name>")
	assert.Contains(t, xmlStr, `<memory unit="MiB">2048</memory>`)
	assert.Contains(t, xmlStr, `<vcpu placement="static">2</vcpu>`)
	assert.Contains(t, xmlStr, `<type arch="x86_64" machine="q35">hvm</type>`)
	assert.Contains(t, xmlStr, `<cpu mode="host-passthrough">`)
	assert.Contains(t, xmlStr, `<emulator>/usr/bin/qemu-system-x86_64</emulator>`)
	assert.Contains(t, xmlStr, `file="/var/lib/libvirt/images/test.qcow2"`)
}

func TestDecodeBase64OrString(t *testing.T) {
	tests := []struct {
		name     string
		input    string
		expected string
	}{
		{
			name:     "empty string",
			input:    "",
			expected: "",
		},
		{
			name:     "valid base64",
			input:    "SGVsbG8gV29ybGQh", // "Hello World!"
			expected: "Hello World!",
		},
		{
			name:     "invalid base64 returns original",
			input:    "not-valid-base64!!!",
			expected: "not-valid-base64!!!",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := decodeBase64OrString(tt.input)
			assert.Equal(t, tt.expected, result)
		})
	}
}

func TestTailFile(t *testing.T) {
	// Create a temporary file with content
	tempFile, err := os.CreateTemp("", "test-log-*.txt")
	require.NoError(t, err)
	defer os.Remove(tempFile.Name())

	content := "line1\nline2\nline3\nline4\nline5\n"
	_, err = tempFile.WriteString(content)
	require.NoError(t, err)
	tempFile.Close()

	// Open file for reading
	file, err := os.Open(tempFile.Name())
	require.NoError(t, err)

	// Get last 2 lines
	reader, err := tailFile(file, 2)
	require.NoError(t, err)
	defer reader.Close()

	buf := make([]byte, 100)
	n, _ := reader.Read(buf)
	result := string(buf[:n])

	// Should contain line4 and line5
	assert.Contains(t, result, "line4")
	assert.Contains(t, result, "line5")
}

func TestQEMUAdapter_Close(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp/images", "/tmp/logs")
	err := adapter.Close()
	assert.NoError(t, err)
}

func TestGuestAgentCommandJSON(t *testing.T) {
	cmd := GuestAgentCommand{
		Execute: "guest-exec",
		Arguments: GuestExecArgs{
			Path:          "/bin/echo",
			Arg:           []string{"hello", "world"},
			CaptureOutput: true,
		},
	}

	// This tests that our JSON structures are correct
	assert.Equal(t, "guest-exec", cmd.Execute)
	args := cmd.Arguments.(GuestExecArgs)
	assert.Equal(t, "/bin/echo", args.Path)
	assert.Equal(t, []string{"hello", "world"}, args.Arg)
	assert.True(t, args.CaptureOutput)
}

func TestQEMUAdapter_GetLogs_NotFound(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	ctx := context.Background()
	_, err := adapter.GetLogs(ctx, "nonexistent-vm", LogOptions{})

	assert.Error(t, err)
	assert.Contains(t, err.Error(), "console log not found")
}

func TestQEMUAdapter_GetLogs_Success(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	// Create a console log file
	logContent := "Boot log line 1\nBoot log line 2\nKernel ready\n"
	logPath := filepath.Join(tempDir, "test-vm.log")
	err := os.WriteFile(logPath, []byte(logContent), 0644)
	require.NoError(t, err)

	ctx := context.Background()
	reader, err := adapter.GetLogs(ctx, "test-vm", LogOptions{})
	require.NoError(t, err)
	defer reader.Close()

	buf := make([]byte, 1024)
	n, _ := reader.Read(buf)
	result := string(buf[:n])

	assert.Contains(t, result, "Boot log line 1")
	assert.Contains(t, result, "Kernel ready")
}

func TestQEMUAdapter_GetLogs_WithTail(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	// Create a console log file with multiple lines
	logContent := "line1\nline2\nline3\nline4\nline5\n"
	logPath := filepath.Join(tempDir, "test-vm.log")
	err := os.WriteFile(logPath, []byte(logContent), 0644)
	require.NoError(t, err)

	ctx := context.Background()
	reader, err := adapter.GetLogs(ctx, "test-vm", LogOptions{Tail: "2"})
	require.NoError(t, err)
	defer reader.Close()

	buf := make([]byte, 1024)
	n, _ := reader.Read(buf)
	result := string(buf[:n])

	assert.Contains(t, result, "line4")
	assert.Contains(t, result, "line5")
}

func TestQEMUAdapter_Exec_EmptyCommand(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp", "/tmp")
	ctx := context.Background()

	_, err := adapter.Exec(ctx, "test-vm", []string{})
	assert.Error(t, err)
	assert.Contains(t, err.Error(), "command cannot be empty")
}

func TestResolveImagePath_FullPath(t *testing.T) {
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", "/tmp/images", "/tmp/logs")

	// Full path should be returned as-is
	result := adapter.resolveImagePath("/mnt/ops/base-images/test.qcow2")
	assert.Equal(t, "/mnt/ops/base-images/test.qcow2", result)
}

func TestResolveImagePath_UbuntuShorthand(t *testing.T) {
	// Create temp directories with a test image
	tempDir := t.TempDir()
	baseDir := filepath.Join(tempDir, "base-images")
	require.NoError(t, os.MkdirAll(baseDir, 0755))

	// Create a test image file
	testImage := filepath.Join(baseDir, "ubuntu-server-24.04-agent.qcow2")
	require.NoError(t, os.WriteFile(testImage, []byte("test"), 0644))

	// Create adapter pointing to temp dirs
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	// Ubuntu shorthand with existing image in base dir
	// Note: This test uses local imagesDir since we can't mock DefaultBaseImagesDir
	localImage := filepath.Join(tempDir, "ubuntu-server-24.04-agent.qcow2")
	require.NoError(t, os.WriteFile(localImage, []byte("test"), 0644))

	result := adapter.resolveImagePath("ubuntu-24.04")
	// Should resolve to the local path since that's where the image exists
	assert.Contains(t, result, "ubuntu-server-24.04-agent.qcow2")
}

func TestResolveImagePath_SimpleName(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	// Create test image
	testImage := filepath.Join(tempDir, "my-custom-image.qcow2")
	require.NoError(t, os.WriteFile(testImage, []byte("test"), 0644))

	// Simple name should have .qcow2 appended
	result := adapter.resolveImagePath("my-custom-image")
	assert.Equal(t, testImage, result)
}

func TestResolveImagePath_RelativeQcow2(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	// Create test image
	testImage := filepath.Join(tempDir, "relative.qcow2")
	require.NoError(t, os.WriteFile(testImage, []byte("test"), 0644))

	// Relative .qcow2 path should be checked in imagesDir
	result := adapter.resolveImagePath("relative.qcow2")
	assert.Equal(t, testImage, result)
}

func TestResolveImagePath_UbuntuVersionFormats(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	tests := []struct {
		input          string
		expectedSuffix string
	}{
		{"ubuntu-22.04", "ubuntu-server-22.04-agent.qcow2"},
		{"ubuntu-24.04", "ubuntu-server-24.04-agent.qcow2"},
		{"ubuntu-25.10", "ubuntu-server-25.10-agent.qcow2"},
	}

	for _, tt := range tests {
		t.Run(tt.input, func(t *testing.T) {
			result := adapter.resolveImagePath(tt.input)
			assert.True(t, strings.HasSuffix(result, tt.expectedSuffix),
				"expected %s to end with %s", result, tt.expectedSuffix)
		})
	}
}

func TestResolveImagePath_NonUbuntuWithPrefix(t *testing.T) {
	tempDir := t.TempDir()
	adapter := NewQEMUAdapterWithPaths("/usr/bin/virsh", tempDir, tempDir)

	// Create a test image that starts with "ubuntu-" but isn't a version shorthand
	testImage := filepath.Join(tempDir, "ubuntu-custom-agent.qcow2")
	require.NoError(t, os.WriteFile(testImage, []byte("test"), 0644))

	// "ubuntu-custom-agent" should NOT be converted (no -agent suffix check)
	result := adapter.resolveImagePath("ubuntu-custom-agent")
	// Since it doesn't match the XX.XX version format, it falls through to default
	assert.Contains(t, result, "ubuntu-custom-agent.qcow2")
}

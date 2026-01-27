package runtime

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"encoding/xml"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

// QEMUAdapter implements RuntimeAdapter for QEMU/libvirt VMs
type QEMUAdapter struct {
	virshPath    string
	imagesDir    string
	consoleLogsDir string
}

// NewQEMUAdapter creates a new QEMU runtime adapter
func NewQEMUAdapter() (*QEMUAdapter, error) {
	// Check if virsh is available
	virshPath, err := exec.LookPath("virsh")
	if err != nil {
		return nil, fmt.Errorf("virsh not found: %w", err)
	}

	// Create default directories
	homeDir, err := os.UserHomeDir()
	if err != nil {
		homeDir = "/var/lib"
	}
	imagesDir := filepath.Join(homeDir, ".agentic-sandbox", "qemu", "images")
	consoleLogsDir := filepath.Join(homeDir, ".agentic-sandbox", "qemu", "logs")

	if err := os.MkdirAll(imagesDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create images directory: %w", err)
	}
	if err := os.MkdirAll(consoleLogsDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create console logs directory: %w", err)
	}

	return &QEMUAdapter{
		virshPath:      virshPath,
		imagesDir:      imagesDir,
		consoleLogsDir: consoleLogsDir,
	}, nil
}

// NewQEMUAdapterWithPaths creates a QEMU adapter with custom paths (for testing)
func NewQEMUAdapterWithPaths(virshPath, imagesDir, consoleLogsDir string) *QEMUAdapter {
	return &QEMUAdapter{
		virshPath:      virshPath,
		imagesDir:      imagesDir,
		consoleLogsDir: consoleLogsDir,
	}
}

// DefaultBaseImagesDir is the default location for agent base images
const DefaultBaseImagesDir = "/mnt/ops/base-images"

// resolveImagePath resolves an image name to a full path
// Supports multiple naming conventions:
// - Full path: "/path/to/image.qcow2" -> used directly
// - Versioned shorthand: "ubuntu-24.04" -> "ubuntu-server-24.04-agent.qcow2"
// - Direct name: "my-image" -> "my-image.qcow2"
func (q *QEMUAdapter) resolveImagePath(imageName string) string {
	// If already a full path with .qcow2 extension, use directly
	if strings.HasSuffix(imageName, ".qcow2") {
		if filepath.IsAbs(imageName) {
			return imageName
		}
		// Relative .qcow2 path - check imagesDir first, then base images dir
		localPath := filepath.Join(q.imagesDir, imageName)
		if _, err := os.Stat(localPath); err == nil {
			return localPath
		}
		basePath := filepath.Join(DefaultBaseImagesDir, imageName)
		if _, err := os.Stat(basePath); err == nil {
			return basePath
		}
		// Fall back to local path (will fail later with clear error)
		return localPath
	}

	// Handle Ubuntu version shorthand: "ubuntu-XX.XX" -> "ubuntu-server-XX.XX-agent.qcow2"
	if strings.HasPrefix(imageName, "ubuntu-") && !strings.Contains(imageName, "-agent") {
		version := strings.TrimPrefix(imageName, "ubuntu-")
		// Validate version format (XX.XX)
		if len(version) >= 4 && version[2] == '.' {
			agentImageName := fmt.Sprintf("ubuntu-server-%s-agent.qcow2", version)
			// Check base images dir first (where build script outputs)
			basePath := filepath.Join(DefaultBaseImagesDir, agentImageName)
			if _, err := os.Stat(basePath); err == nil {
				return basePath
			}
			// Check local images dir
			localPath := filepath.Join(q.imagesDir, agentImageName)
			if _, err := os.Stat(localPath); err == nil {
				return localPath
			}
			// Return base path (will fail later with clear error)
			return basePath
		}
	}

	// Default: append .qcow2 and look in images directories
	imagePath := imageName + ".qcow2"

	// Check local images dir first
	localPath := filepath.Join(q.imagesDir, imagePath)
	if _, err := os.Stat(localPath); err == nil {
		return localPath
	}

	// Check base images dir
	basePath := filepath.Join(DefaultBaseImagesDir, imagePath)
	if _, err := os.Stat(basePath); err == nil {
		return basePath
	}

	// Fall back to local path
	return localPath
}

// LibvirtDomain represents a libvirt domain XML structure
type LibvirtDomain struct {
	XMLName     xml.Name           `xml:"domain"`
	Type        string             `xml:"type,attr"`
	Name        string             `xml:"name"`
	UUID        string             `xml:"uuid,omitempty"`
	Memory      LibvirtMemory      `xml:"memory"`
	VCPU        LibvirtVCPU        `xml:"vcpu"`
	OS          LibvirtOS          `xml:"os"`
	Features    LibvirtFeatures    `xml:"features"`
	CPU         LibvirtCPU         `xml:"cpu"`
	Devices     LibvirtDevices     `xml:"devices"`
	OnPoweroff  string             `xml:"on_poweroff"`
	OnReboot    string             `xml:"on_reboot"`
	OnCrash     string             `xml:"on_crash"`
}

type LibvirtMemory struct {
	Unit  string `xml:"unit,attr"`
	Value int64  `xml:",chardata"`
}

type LibvirtVCPU struct {
	Placement string `xml:"placement,attr,omitempty"`
	Value     int    `xml:",chardata"`
}

type LibvirtOS struct {
	Type LibvirtOSType `xml:"type"`
	Boot LibvirtBoot   `xml:"boot"`
}

type LibvirtOSType struct {
	Arch    string `xml:"arch,attr,omitempty"`
	Machine string `xml:"machine,attr,omitempty"`
	Value   string `xml:",chardata"`
}

type LibvirtBoot struct {
	Dev string `xml:"dev,attr"`
}

type LibvirtFeatures struct {
	ACPI struct{} `xml:"acpi"`
	APIC struct{} `xml:"apic"`
}

type LibvirtCPU struct {
	Mode string `xml:"mode,attr"`
}

type LibvirtDevices struct {
	Emulator    string                `xml:"emulator"`
	Disks       []LibvirtDisk         `xml:"disk"`
	Interfaces  []LibvirtInterface    `xml:"interface"`
	Serials     []LibvirtSerial       `xml:"serial"`
	Consoles    []LibvirtConsole      `xml:"console"`
	Channels    []LibvirtChannel      `xml:"channel"`
	Graphics    []LibvirtGraphics     `xml:"graphics,omitempty"`
	Filesystems []LibvirtFilesystem   `xml:"filesystem,omitempty"`
}

type LibvirtDisk struct {
	Type   string            `xml:"type,attr"`
	Device string            `xml:"device,attr"`
	Driver LibvirtDiskDriver `xml:"driver"`
	Source LibvirtDiskSource `xml:"source"`
	Target LibvirtDiskTarget `xml:"target"`
}

type LibvirtDiskDriver struct {
	Name string `xml:"name,attr"`
	Type string `xml:"type,attr"`
}

type LibvirtDiskSource struct {
	File string `xml:"file,attr,omitempty"`
}

type LibvirtDiskTarget struct {
	Dev string `xml:"dev,attr"`
	Bus string `xml:"bus,attr"`
}

type LibvirtInterface struct {
	Type   string               `xml:"type,attr"`
	Source LibvirtInterfaceSource `xml:"source,omitempty"`
	Model  LibvirtInterfaceModel  `xml:"model"`
}

type LibvirtInterfaceSource struct {
	Network string `xml:"network,attr,omitempty"`
}

type LibvirtInterfaceModel struct {
	Type string `xml:"type,attr"`
}

type LibvirtSerial struct {
	Type   string              `xml:"type,attr"`
	Source *LibvirtSerialSource `xml:"source,omitempty"`
	Target LibvirtSerialTarget `xml:"target"`
	Log    *LibvirtSerialLog   `xml:"log,omitempty"`
}

type LibvirtSerialSource struct {
	Path string `xml:"path,attr,omitempty"`
}

type LibvirtSerialTarget struct {
	Port int `xml:"port,attr"`
}

type LibvirtSerialLog struct {
	File   string `xml:"file,attr"`
	Append string `xml:"append,attr"`
}

type LibvirtConsole struct {
	Type   string              `xml:"type,attr"`
	Target LibvirtConsoleTarget `xml:"target"`
}

type LibvirtConsoleTarget struct {
	Type string `xml:"type,attr"`
	Port int    `xml:"port,attr"`
}

type LibvirtChannel struct {
	Type   string               `xml:"type,attr"`
	Target LibvirtChannelTarget `xml:"target"`
}

type LibvirtChannelTarget struct {
	Type string `xml:"type,attr"`
	Name string `xml:"name,attr"`
}

type LibvirtGraphics struct {
	Type     string `xml:"type,attr"`
	Port     string `xml:"port,attr"`
	AutoPort string `xml:"autoport,attr"`
}

type LibvirtFilesystem struct {
	Type       string                    `xml:"type,attr"`
	AccessMode string                    `xml:"accessmode,attr"`
	Source     LibvirtFilesystemSource   `xml:"source"`
	Target     LibvirtFilesystemTarget   `xml:"target"`
	Readonly   *struct{}                 `xml:"readonly,omitempty"`
}

type LibvirtFilesystemSource struct {
	Dir string `xml:"dir,attr"`
}

type LibvirtFilesystemTarget struct {
	Dir string `xml:"dir,attr"`
}

// Create creates a new VM from the specification
func (q *QEMUAdapter) Create(ctx context.Context, spec *SandboxSpec) (string, error) {
	// Generate VM ID
	vmID := fmt.Sprintf("sandbox-%s-%d", spec.Name, time.Now().UnixNano())

	// Create disk image (copy from base image or create new)
	diskPath := filepath.Join(q.imagesDir, vmID+".qcow2")
	consoleLogPath := filepath.Join(q.consoleLogsDir, vmID+".log")

	// Resolve base image path (supports shorthand like "ubuntu-24.04")
	baseImagePath := q.resolveImagePath(spec.Image)
	if _, err := os.Stat(baseImagePath); err == nil {
		// Clone base image
		cmd := exec.CommandContext(ctx, "qemu-img", "create", "-f", "qcow2",
			"-b", baseImagePath, "-F", "qcow2", diskPath)
		if output, err := cmd.CombinedOutput(); err != nil {
			return "", fmt.Errorf("failed to create disk image: %w: %s", err, output)
		}
	} else {
		// Create blank disk with specified size
		diskSize := "20G"
		if spec.Resources.DiskQuotaGB > 0 {
			diskSize = fmt.Sprintf("%dG", spec.Resources.DiskQuotaGB)
		}
		cmd := exec.CommandContext(ctx, "qemu-img", "create", "-f", "qcow2", diskPath, diskSize)
		if output, err := cmd.CombinedOutput(); err != nil {
			return "", fmt.Errorf("failed to create disk image: %w: %s", err, output)
		}
	}

	// Generate libvirt domain XML
	domain := q.buildDomainXML(vmID, spec, diskPath, consoleLogPath)

	xmlData, err := xml.MarshalIndent(domain, "", "  ")
	if err != nil {
		return "", fmt.Errorf("failed to generate domain XML: %w", err)
	}

	// Write XML to temporary file
	xmlPath := filepath.Join(q.imagesDir, vmID+".xml")
	if err := os.WriteFile(xmlPath, xmlData, 0644); err != nil {
		return "", fmt.Errorf("failed to write domain XML: %w", err)
	}

	// Define the VM
	cmd := exec.CommandContext(ctx, q.virshPath, "define", xmlPath)
	if output, err := cmd.CombinedOutput(); err != nil {
		// Clean up disk image on failure
		os.Remove(diskPath)
		os.Remove(xmlPath)
		return "", fmt.Errorf("failed to define VM: %w: %s", err, output)
	}

	return vmID, nil
}

// buildDomainXML creates a libvirt domain XML from the sandbox spec
func (q *QEMUAdapter) buildDomainXML(vmID string, spec *SandboxSpec, diskPath, consoleLogPath string) LibvirtDomain {
	// Calculate vCPUs (round up CPUs to whole number)
	vcpus := int(spec.Resources.CPUs)
	if vcpus < 1 {
		vcpus = 1
	}

	domain := LibvirtDomain{
		Type: "kvm",
		Name: vmID,
		Memory: LibvirtMemory{
			Unit:  "MiB",
			Value: spec.Resources.MemoryMB,
		},
		VCPU: LibvirtVCPU{
			Placement: "static",
			Value:     vcpus,
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
					Source: LibvirtDiskSource{File: diskPath},
					Target: LibvirtDiskTarget{Dev: "vda", Bus: "virtio"},
				},
			},
			Serials: []LibvirtSerial{
				{
					Type:   "file",
					Source: &LibvirtSerialSource{Path: consoleLogPath},
					Target: LibvirtSerialTarget{Port: 0},
					Log:    &LibvirtSerialLog{File: consoleLogPath, Append: "on"},
				},
			},
			Consoles: []LibvirtConsole{
				{
					Type:   "pty",
					Target: LibvirtConsoleTarget{Type: "serial", Port: 0},
				},
			},
			Channels: []LibvirtChannel{
				{
					Type:   "unix",
					Target: LibvirtChannelTarget{Type: "virtio", Name: "org.qemu.guest_agent.0"},
				},
			},
		},
		OnPoweroff: "destroy",
		OnReboot:   "restart",
		OnCrash:    "destroy",
	}

	// Configure networking
	switch spec.Network.Mode {
	case "none":
		// No network interface
	case "host":
		domain.Devices.Interfaces = []LibvirtInterface{
			{
				Type:  "bridge",
				Source: LibvirtInterfaceSource{Network: "default"},
				Model: LibvirtInterfaceModel{Type: "virtio"},
			},
		}
	default: // "bridge" or default
		domain.Devices.Interfaces = []LibvirtInterface{
			{
				Type:  "network",
				Source: LibvirtInterfaceSource{Network: "default"},
				Model: LibvirtInterfaceModel{Type: "virtio"},
			},
		}
	}

	// Add filesystem mounts (9p virtio)
	for _, mount := range spec.Mounts {
		fs := LibvirtFilesystem{
			Type:       "mount",
			AccessMode: "mapped",
			Source:     LibvirtFilesystemSource{Dir: mount.Source},
			Target:     LibvirtFilesystemTarget{Dir: mount.Target},
		}
		if mount.ReadOnly {
			fs.Readonly = &struct{}{}
		}
		domain.Devices.Filesystems = append(domain.Devices.Filesystems, fs)
	}

	return domain
}

// Start starts a stopped VM
func (q *QEMUAdapter) Start(ctx context.Context, sandboxID string) error {
	cmd := exec.CommandContext(ctx, q.virshPath, "start", sandboxID)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("failed to start VM: %w: %s", err, output)
	}
	return nil
}

// Stop stops a running VM
func (q *QEMUAdapter) Stop(ctx context.Context, sandboxID string) error {
	cmd := exec.CommandContext(ctx, q.virshPath, "shutdown", sandboxID)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("failed to stop VM: %w: %s", err, output)
	}
	return nil
}

// ForceStop forcefully stops a running VM
func (q *QEMUAdapter) ForceStop(ctx context.Context, sandboxID string) error {
	cmd := exec.CommandContext(ctx, q.virshPath, "destroy", sandboxID)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("failed to force stop VM: %w: %s", err, output)
	}
	return nil
}

// Delete removes a VM and its resources
func (q *QEMUAdapter) Delete(ctx context.Context, sandboxID string) error {
	// First destroy (force stop) if running
	cmd := exec.CommandContext(ctx, q.virshPath, "destroy", sandboxID)
	cmd.Run() // Ignore error if already stopped

	// Then undefine (delete)
	cmd = exec.CommandContext(ctx, q.virshPath, "undefine", sandboxID, "--remove-all-storage")
	output, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("failed to delete VM: %w: %s", err, output)
	}

	// Clean up console log
	consoleLogPath := filepath.Join(q.consoleLogsDir, sandboxID+".log")
	os.Remove(consoleLogPath)

	// Clean up XML file
	xmlPath := filepath.Join(q.imagesDir, sandboxID+".xml")
	os.Remove(xmlPath)

	return nil
}

// GuestAgentCommand represents a command to send via qemu-guest-agent
type GuestAgentCommand struct {
	Execute   string      `json:"execute"`
	Arguments interface{} `json:"arguments,omitempty"`
}

// GuestExecArgs represents arguments for guest-exec command
type GuestExecArgs struct {
	Path       string   `json:"path"`
	Arg        []string `json:"arg,omitempty"`
	CaptureOutput bool  `json:"capture-output"`
}

// GuestExecResponse represents the response from guest-exec
type GuestExecResponse struct {
	Return struct {
		PID int `json:"pid"`
	} `json:"return"`
}

// GuestExecStatusArgs represents arguments for guest-exec-status
type GuestExecStatusArgs struct {
	PID int `json:"pid"`
}

// GuestExecStatusResponse represents the response from guest-exec-status
type GuestExecStatusResponse struct {
	Return struct {
		Exited   bool   `json:"exited"`
		ExitCode int    `json:"exitcode,omitempty"`
		OutData  string `json:"out-data,omitempty"`
		ErrData  string `json:"err-data,omitempty"`
	} `json:"return"`
}

// Exec executes a command in the VM via qemu-guest-agent
func (q *QEMUAdapter) Exec(ctx context.Context, sandboxID string, cmd []string) (*ExecResult, error) {
	if len(cmd) == 0 {
		return nil, fmt.Errorf("command cannot be empty")
	}

	// Build guest-exec command
	execCmd := GuestAgentCommand{
		Execute: "guest-exec",
		Arguments: GuestExecArgs{
			Path:          cmd[0],
			Arg:           cmd[1:],
			CaptureOutput: true,
		},
	}

	cmdJSON, err := json.Marshal(execCmd)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal exec command: %w", err)
	}

	// Execute via virsh qemu-agent-command
	virshCmd := exec.CommandContext(ctx, q.virshPath, "qemu-agent-command", sandboxID, string(cmdJSON))
	output, err := virshCmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("failed to execute command via guest agent: %w: %s", err, output)
	}

	// Parse response to get PID
	var execResp GuestExecResponse
	if err := json.Unmarshal(output, &execResp); err != nil {
		return nil, fmt.Errorf("failed to parse exec response: %w: %s", err, output)
	}

	// Poll for completion
	for {
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}

		statusCmd := GuestAgentCommand{
			Execute: "guest-exec-status",
			Arguments: GuestExecStatusArgs{
				PID: execResp.Return.PID,
			},
		}

		statusJSON, err := json.Marshal(statusCmd)
		if err != nil {
			return nil, fmt.Errorf("failed to marshal status command: %w", err)
		}

		virshCmd := exec.CommandContext(ctx, q.virshPath, "qemu-agent-command", sandboxID, string(statusJSON))
		statusOutput, err := virshCmd.CombinedOutput()
		if err != nil {
			return nil, fmt.Errorf("failed to get exec status: %w: %s", err, statusOutput)
		}

		var statusResp GuestExecStatusResponse
		if err := json.Unmarshal(statusOutput, &statusResp); err != nil {
			return nil, fmt.Errorf("failed to parse status response: %w: %s", err, statusOutput)
		}

		if statusResp.Return.Exited {
			return &ExecResult{
				Stdout:   decodeBase64OrString(statusResp.Return.OutData),
				Stderr:   decodeBase64OrString(statusResp.Return.ErrData),
				ExitCode: statusResp.Return.ExitCode,
			}, nil
		}
	}
}

// decodeBase64OrString attempts to decode base64, returns original on failure
func decodeBase64OrString(s string) string {
	if s == "" {
		return ""
	}
	// Guest agent returns base64-encoded output
	decoded, err := base64.StdEncoding.DecodeString(s)
	if err != nil {
		return s
	}
	return string(decoded)
}

// GetStatus returns the current status of a VM
func (q *QEMUAdapter) GetStatus(ctx context.Context, sandboxID string) (*SandboxStatus, error) {
	cmd := exec.CommandContext(ctx, q.virshPath, "dominfo", sandboxID)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("failed to get VM info: %w: %s", err, output)
	}

	// Parse virsh dominfo output
	status := &SandboxStatus{
		ID:      sandboxID,
		Name:    sandboxID,
		Runtime: "qemu",
	}

	lines := strings.Split(string(output), "\n")
	for _, line := range lines {
		parts := strings.SplitN(line, ":", 2)
		if len(parts) != 2 {
			continue
		}
		key := strings.TrimSpace(parts[0])
		value := strings.TrimSpace(parts[1])

		switch key {
		case "State":
			// Map libvirt states to our states
			if strings.Contains(value, "running") {
				status.State = "running"
			} else if strings.Contains(value, "shut off") {
				status.State = "stopped"
			} else if strings.Contains(value, "paused") {
				status.State = "paused"
			} else {
				status.State = value
			}
		case "Max memory":
			// Parse memory (e.g., "8388608 KiB")
			var memKiB int64
			fmt.Sscanf(value, "%d", &memKiB)
			if status.Resources == nil {
				status.Resources = &ResourceUsage{}
			}
			status.Resources.MemoryLimitMB = memKiB / 1024
		}
	}

	return status, nil
}

// List returns all VMs managed by this adapter
func (q *QEMUAdapter) List(ctx context.Context) ([]SandboxInfo, error) {
	// List only VMs with sandbox- prefix
	cmd := exec.CommandContext(ctx, q.virshPath, "list", "--all", "--name")
	output, err := cmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("failed to list VMs: %w: %s", err, output)
	}

	var infos []SandboxInfo
	lines := strings.Split(string(output), "\n")
	for _, line := range lines {
		name := strings.TrimSpace(line)
		if name == "" || !strings.HasPrefix(name, "sandbox-") {
			continue
		}

		// Get status for each VM
		status, err := q.GetStatus(ctx, name)
		if err != nil {
			continue
		}

		infos = append(infos, SandboxInfo{
			ID:      name,
			Name:    name,
			Runtime: "qemu",
			State:   status.State,
		})
	}

	return infos, nil
}

// GetLogs retrieves console logs from a VM
func (q *QEMUAdapter) GetLogs(ctx context.Context, sandboxID string, opts LogOptions) (io.ReadCloser, error) {
	consoleLogPath := filepath.Join(q.consoleLogsDir, sandboxID+".log")

	file, err := os.Open(consoleLogPath)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, fmt.Errorf("console log not found for VM %s", sandboxID)
		}
		return nil, fmt.Errorf("failed to open console log: %w", err)
	}

	// Handle tail option
	if opts.Tail != "" && opts.Tail != "all" {
		var tailLines int
		fmt.Sscanf(opts.Tail, "%d", &tailLines)
		if tailLines > 0 {
			return tailFile(file, tailLines)
		}
	}

	return file, nil
}

// tailFile returns the last n lines of a file
func tailFile(file *os.File, lines int) (io.ReadCloser, error) {
	stat, err := file.Stat()
	if err != nil {
		file.Close()
		return nil, err
	}

	// Read file content
	content := make([]byte, stat.Size())
	_, err = file.Read(content)
	if err != nil && err != io.EOF {
		file.Close()
		return nil, err
	}
	file.Close()

	// Split into lines and filter out empty trailing lines
	allLines := strings.Split(string(content), "\n")

	// Remove trailing empty lines
	for len(allLines) > 0 && allLines[len(allLines)-1] == "" {
		allLines = allLines[:len(allLines)-1]
	}

	// Take last N lines
	start := len(allLines) - lines
	if start < 0 {
		start = 0
	}
	result := strings.Join(allLines[start:], "\n")

	return io.NopCloser(strings.NewReader(result)), nil
}

// Close cleans up adapter resources
func (q *QEMUAdapter) Close() error {
	return nil
}

// WaitForGuestAgent waits for the guest agent to become available
func (q *QEMUAdapter) WaitForGuestAgent(ctx context.Context, sandboxID string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)

	pingCmd := GuestAgentCommand{Execute: "guest-ping"}
	pingJSON, _ := json.Marshal(pingCmd)

	for time.Now().Before(deadline) {
		select {
		case <-ctx.Done():
			return ctx.Err()
		default:
		}

		cmd := exec.CommandContext(ctx, q.virshPath, "qemu-agent-command", sandboxID, string(pingJSON))
		if err := cmd.Run(); err == nil {
			return nil
		}

		time.Sleep(time.Second)
	}

	return fmt.Errorf("guest agent not available after %v", timeout)
}

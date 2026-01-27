// Package testutil provides test data factories for generating dynamic test data
package testutil

import (
	"fmt"
	"math/rand"
	"time"

	"github.com/roctinam/agentic-sandbox/internal/runtime"
)

// SandboxSpecFactory generates SandboxSpec instances for testing
type SandboxSpecFactory struct {
	rng *rand.Rand
}

// NewSandboxSpecFactory creates a new factory
func NewSandboxSpecFactory() *SandboxSpecFactory {
	return &SandboxSpecFactory{
		rng: rand.New(rand.NewSource(time.Now().UnixNano())),
	}
}

// Build creates a SandboxSpec with default values and optional overrides
func (f *SandboxSpecFactory) Build(overrides ...func(*runtime.SandboxSpec)) *runtime.SandboxSpec {
	spec := &runtime.SandboxSpec{
		Name:      fmt.Sprintf("test-sandbox-%d", f.rng.Int63()),
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: runtime.DefaultResourceLimits(),
		Network:   runtime.DefaultNetworkConfig(),
		Security:  runtime.DefaultSecurityConfig(),
		Env:       make(map[string]string),
		Command:   []string{"sleep", "300"},
	}

	// Apply overrides
	for _, override := range overrides {
		override(spec)
	}

	return spec
}

// BuildMinimal creates a minimal SandboxSpec
func (f *SandboxSpecFactory) BuildMinimal() *runtime.SandboxSpec {
	return &runtime.SandboxSpec{
		Name:      fmt.Sprintf("minimal-%d", f.rng.Int63()),
		Image:     "alpine:3.18",
		Runtime:   "docker",
		Resources: runtime.ResourceLimits{
			CPUs:      1.0,
			MemoryMB:  512,
			PIDsLimit: 64,
		},
		Network:  runtime.NetworkConfig{Mode: "none"},
		Security: runtime.DefaultSecurityConfig(),
	}
}

// BuildWithCustomResources creates a spec with custom resource limits
func (f *SandboxSpecFactory) BuildWithCustomResources(cpus float64, memoryMB int64, pids int64) *runtime.SandboxSpec {
	return f.Build(func(spec *runtime.SandboxSpec) {
		spec.Resources = runtime.ResourceLimits{
			CPUs:      cpus,
			MemoryMB:  memoryMB,
			PIDsLimit: pids,
		}
	})
}

// BuildWithNetwork creates a spec with custom network configuration
func (f *SandboxSpecFactory) BuildWithNetwork(mode string, dns []string) *runtime.SandboxSpec {
	return f.Build(func(spec *runtime.SandboxSpec) {
		spec.Network = runtime.NetworkConfig{
			Mode: mode,
			DNS:  dns,
		}
	})
}

// BuildWithMounts creates a spec with mounts
func (f *SandboxSpecFactory) BuildWithMounts(mounts []runtime.MountConfig) *runtime.SandboxSpec {
	return f.Build(func(spec *runtime.SandboxSpec) {
		spec.Mounts = mounts
	})
}

// BuildWithEnv creates a spec with environment variables
func (f *SandboxSpecFactory) BuildWithEnv(env map[string]string) *runtime.SandboxSpec {
	return f.Build(func(spec *runtime.SandboxSpec) {
		spec.Env = env
	})
}

// BuildHardened creates a maximally hardened spec
// Note: SeccompProfile is intentionally left empty for tests since the profile
// file won't be installed at /etc during testing. Tests can override this if needed.
func (f *SandboxSpecFactory) BuildHardened() *runtime.SandboxSpec {
	return f.Build(func(spec *runtime.SandboxSpec) {
		spec.Network = runtime.NetworkConfig{Mode: "none"}
		spec.Security = runtime.SecurityConfig{
			Privileged:      false,
			ReadOnlyRootFS:  true,
			NoNewPrivileges: true,
			CapDrop:         []string{"ALL"},
			CapAdd:          []string{},
			SeccompProfile:  "", // Empty for tests - use default Docker seccomp
		}
		spec.Resources = runtime.ResourceLimits{
			CPUs:      1.0,
			MemoryMB:  256,
			PIDsLimit: 32,
		}
	})
}

// BuildQEMU creates a QEMU VM spec
func (f *SandboxSpecFactory) BuildQEMU() *runtime.SandboxSpec {
	return f.Build(func(spec *runtime.SandboxSpec) {
		spec.Runtime = "qemu"
		spec.Image = "ubuntu-22.04-agent"
		spec.Resources = runtime.ResourceLimits{
			CPUs:      8.0,
			MemoryMB:  16384,
			PIDsLimit: 2048,
		}
	})
}

// BuildList creates multiple SandboxSpec instances
func (f *SandboxSpecFactory) BuildList(count int) []*runtime.SandboxSpec {
	specs := make([]*runtime.SandboxSpec, count)
	for i := 0; i < count; i++ {
		specs[i] = f.Build()
	}
	return specs
}

// ExecResultFactory generates ExecResult instances for testing
type ExecResultFactory struct{}

// NewExecResultFactory creates a new factory
func NewExecResultFactory() *ExecResultFactory {
	return &ExecResultFactory{}
}

// Build creates an ExecResult with optional overrides
func (f *ExecResultFactory) Build(overrides ...func(*runtime.ExecResult)) *runtime.ExecResult {
	result := &runtime.ExecResult{
		Stdout:   "success",
		Stderr:   "",
		ExitCode: 0,
	}

	for _, override := range overrides {
		override(result)
	}

	return result
}

// BuildError creates a failed ExecResult
func (f *ExecResultFactory) BuildError(exitCode int, stderr string) *runtime.ExecResult {
	return &runtime.ExecResult{
		Stdout:   "",
		Stderr:   stderr,
		ExitCode: exitCode,
	}
}

// BuildSuccess creates a successful ExecResult with output
func (f *ExecResultFactory) BuildSuccess(stdout string) *runtime.ExecResult {
	return &runtime.ExecResult{
		Stdout:   stdout,
		Stderr:   "",
		ExitCode: 0,
	}
}

// SandboxStatusFactory generates SandboxStatus instances for testing
type SandboxStatusFactory struct {
	rng *rand.Rand
}

// NewSandboxStatusFactory creates a new factory
func NewSandboxStatusFactory() *SandboxStatusFactory {
	return &SandboxStatusFactory{
		rng: rand.New(rand.NewSource(time.Now().UnixNano())),
	}
}

// Build creates a SandboxStatus with optional overrides
func (f *SandboxStatusFactory) Build(overrides ...func(*runtime.SandboxStatus)) *runtime.SandboxStatus {
	status := &runtime.SandboxStatus{
		ID:      fmt.Sprintf("sandbox-%d", f.rng.Int63()),
		Name:    fmt.Sprintf("test-sandbox-%d", f.rng.Int63()),
		State:   "running",
		Runtime: "docker",
		Resources: &runtime.ResourceUsage{
			CPUPercent:    25.5,
			MemoryUsageMB: 512,
			MemoryLimitMB: 8192,
			PIDs:          10,
		},
	}

	for _, override := range overrides {
		override(status)
	}

	return status
}

// BuildStopped creates a stopped sandbox status
func (f *SandboxStatusFactory) BuildStopped() *runtime.SandboxStatus {
	return f.Build(func(status *runtime.SandboxStatus) {
		status.State = "stopped"
		status.ExitCode = 0
		status.Resources = nil
	})
}

// BuildError creates an error sandbox status
func (f *SandboxStatusFactory) BuildError(err string) *runtime.SandboxStatus {
	return f.Build(func(status *runtime.SandboxStatus) {
		status.State = "error"
		status.Error = err
		status.ExitCode = 1
		status.Resources = nil
	})
}

// MountConfigFactory generates mount configurations
type MountConfigFactory struct{}

// NewMountConfigFactory creates a new factory
func NewMountConfigFactory() *MountConfigFactory {
	return &MountConfigFactory{}
}

// BuildBind creates a bind mount
func (f *MountConfigFactory) BuildBind(source, target string, readOnly bool) runtime.MountConfig {
	return runtime.MountConfig{
		Source:   source,
		Target:   target,
		Type:     "bind",
		ReadOnly: readOnly,
	}
}

// BuildTmpfs creates a tmpfs mount
func (f *MountConfigFactory) BuildTmpfs(target string) runtime.MountConfig {
	return runtime.MountConfig{
		Source:   "",
		Target:   target,
		Type:     "tmpfs",
		ReadOnly: false,
	}
}

// BuildVolume creates a volume mount
func (f *MountConfigFactory) BuildVolume(volume, target string, readOnly bool) runtime.MountConfig {
	return runtime.MountConfig{
		Source:   volume,
		Target:   target,
		Type:     "volume",
		ReadOnly: readOnly,
	}
}

// RandomString generates a random string of given length
func RandomString(length int) string {
	const charset = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
	rng := rand.New(rand.NewSource(time.Now().UnixNano()))
	b := make([]byte, length)
	for i := range b {
		b[i] = charset[rng.Intn(len(charset))]
	}
	return string(b)
}

// RandomInt generates a random integer between min and max
func RandomInt(min, max int) int {
	rng := rand.New(rand.NewSource(time.Now().UnixNano()))
	return min + rng.Intn(max-min+1)
}

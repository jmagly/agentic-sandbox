package config

import (
	"fmt"
	"os"
	"strconv"
)

// Config holds application configuration
type Config struct {
	Server   ServerConfig
	Docker   DockerConfig
	QEMU     QEMUConfig
	Security SecurityConfig
}

// ServerConfig holds HTTP server configuration
type ServerConfig struct {
	Host string
	Port int
}

// DockerConfig holds Docker-specific configuration
type DockerConfig struct {
	SeccompProfile string
	DefaultNetwork string
}

// QEMUConfig holds QEMU/libvirt configuration
type QEMUConfig struct {
	LibvirtURI    string
	TemplatesPath string
}

// SecurityConfig holds security-related configuration
type SecurityConfig struct {
	EnableSeccomp     bool
	EnableAppArmor    bool
	DefaultPidsLimit  int
	DefaultMemoryMB   int
	DefaultCPUs       int
}

// Load loads configuration from environment variables
func Load() (*Config, error) {
	cfg := &Config{
		Server: ServerConfig{
			Host: getEnv("SERVER_HOST", "0.0.0.0"),
			Port: getEnvInt("SERVER_PORT", 8080),
		},
		Docker: DockerConfig{
			SeccompProfile: getEnv("DOCKER_SECCOMP_PROFILE", "/etc/agentic-sandbox/seccomp-agent.json"),
			DefaultNetwork: getEnv("DOCKER_DEFAULT_NETWORK", "isolated"),
		},
		QEMU: QEMUConfig{
			LibvirtURI:    getEnv("QEMU_LIBVIRT_URI", "qemu:///system"),
			TemplatesPath: getEnv("QEMU_TEMPLATES_PATH", "/etc/agentic-sandbox/qemu"),
		},
		Security: SecurityConfig{
			EnableSeccomp:    getEnvBool("SECURITY_ENABLE_SECCOMP", true),
			EnableAppArmor:   getEnvBool("SECURITY_ENABLE_APPARMOR", false),
			DefaultPidsLimit: getEnvInt("SECURITY_DEFAULT_PIDS_LIMIT", 1024),
			DefaultMemoryMB:  getEnvInt("SECURITY_DEFAULT_MEMORY_MB", 8192),
			DefaultCPUs:      getEnvInt("SECURITY_DEFAULT_CPUS", 4),
		},
	}

	return cfg, nil
}

// Validate validates the configuration
func (c *Config) Validate() error {
	if c.Server.Port < 1 || c.Server.Port > 65535 {
		return fmt.Errorf("invalid server port: %d", c.Server.Port)
	}

	if c.Security.DefaultPidsLimit < 1 {
		return fmt.Errorf("invalid default pids limit: %d", c.Security.DefaultPidsLimit)
	}

	if c.Security.DefaultMemoryMB < 128 {
		return fmt.Errorf("invalid default memory: %d MB (minimum 128 MB)", c.Security.DefaultMemoryMB)
	}

	if c.Security.DefaultCPUs < 1 {
		return fmt.Errorf("invalid default CPUs: %d", c.Security.DefaultCPUs)
	}

	return nil
}

// getEnv returns environment variable value or default
func getEnv(key, defaultValue string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}
	return defaultValue
}

// getEnvInt returns environment variable as integer or default
func getEnvInt(key string, defaultValue int) int {
	if value := os.Getenv(key); value != "" {
		if intVal, err := strconv.Atoi(value); err == nil {
			return intVal
		}
	}
	return defaultValue
}

// getEnvBool returns environment variable as boolean or default
func getEnvBool(key string, defaultValue bool) bool {
	if value := os.Getenv(key); value != "" {
		if boolVal, err := strconv.ParseBool(value); err == nil {
			return boolVal
		}
	}
	return defaultValue
}

package config

import (
	"os"
	"testing"
)

func TestLoad(t *testing.T) {
	cfg, err := Load()
	if err != nil {
		t.Fatalf("expected no error loading config, got %v", err)
	}
	if cfg == nil {
		t.Fatal("expected config to be non-nil")
	}
}

func TestDefaultValues(t *testing.T) {
	cfg, _ := Load()

	if cfg.Server.Host != "0.0.0.0" {
		t.Errorf("expected default host '0.0.0.0', got '%s'", cfg.Server.Host)
	}
	if cfg.Server.Port != 8080 {
		t.Errorf("expected default port 8080, got %d", cfg.Server.Port)
	}
	if cfg.Security.DefaultPidsLimit != 1024 {
		t.Errorf("expected default pids limit 1024, got %d", cfg.Security.DefaultPidsLimit)
	}
	if cfg.Security.EnableSeccomp != true {
		t.Error("expected seccomp to be enabled by default")
	}
}

func TestEnvironmentOverrides(t *testing.T) {
	os.Setenv("SERVER_PORT", "9090")
	os.Setenv("SECURITY_DEFAULT_PIDS_LIMIT", "512")
	defer func() {
		os.Unsetenv("SERVER_PORT")
		os.Unsetenv("SECURITY_DEFAULT_PIDS_LIMIT")
	}()

	cfg, _ := Load()

	if cfg.Server.Port != 9090 {
		t.Errorf("expected port 9090 from env, got %d", cfg.Server.Port)
	}
	if cfg.Security.DefaultPidsLimit != 512 {
		t.Errorf("expected pids limit 512 from env, got %d", cfg.Security.DefaultPidsLimit)
	}
}

func TestValidate(t *testing.T) {
	tests := []struct {
		name    string
		cfg     *Config
		wantErr bool
	}{
		{
			name: "valid config",
			cfg: &Config{
				Server: ServerConfig{Port: 8080},
				Security: SecurityConfig{
					DefaultPidsLimit: 1024,
					DefaultMemoryMB:  8192,
					DefaultCPUs:      4,
				},
			},
			wantErr: false,
		},
		{
			name: "invalid port",
			cfg: &Config{
				Server: ServerConfig{Port: 99999},
				Security: SecurityConfig{
					DefaultPidsLimit: 1024,
					DefaultMemoryMB:  8192,
					DefaultCPUs:      4,
				},
			},
			wantErr: true,
		},
		{
			name: "invalid pids limit",
			cfg: &Config{
				Server: ServerConfig{Port: 8080},
				Security: SecurityConfig{
					DefaultPidsLimit: 0,
					DefaultMemoryMB:  8192,
					DefaultCPUs:      4,
				},
			},
			wantErr: true,
		},
		{
			name: "invalid memory",
			cfg: &Config{
				Server: ServerConfig{Port: 8080},
				Security: SecurityConfig{
					DefaultPidsLimit: 1024,
					DefaultMemoryMB:  64,
					DefaultCPUs:      4,
				},
			},
			wantErr: true,
		},
		{
			name: "invalid cpus",
			cfg: &Config{
				Server: ServerConfig{Port: 8080},
				Security: SecurityConfig{
					DefaultPidsLimit: 1024,
					DefaultMemoryMB:  8192,
					DefaultCPUs:      0,
				},
			},
			wantErr: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := tt.cfg.Validate()
			if (err != nil) != tt.wantErr {
				t.Errorf("expected error=%v, got error=%v", tt.wantErr, err)
			}
		})
	}
}

func TestGetEnv(t *testing.T) {
	os.Setenv("TEST_VAR", "test_value")
	defer os.Unsetenv("TEST_VAR")

	value := getEnv("TEST_VAR", "default")
	if value != "test_value" {
		t.Errorf("expected 'test_value', got '%s'", value)
	}

	value = getEnv("NONEXISTENT", "default")
	if value != "default" {
		t.Errorf("expected 'default', got '%s'", value)
	}
}

func TestGetEnvInt(t *testing.T) {
	os.Setenv("TEST_INT", "42")
	defer os.Unsetenv("TEST_INT")

	value := getEnvInt("TEST_INT", 0)
	if value != 42 {
		t.Errorf("expected 42, got %d", value)
	}

	value = getEnvInt("NONEXISTENT", 99)
	if value != 99 {
		t.Errorf("expected 99, got %d", value)
	}
}

func TestGetEnvBool(t *testing.T) {
	os.Setenv("TEST_BOOL", "true")
	defer os.Unsetenv("TEST_BOOL")

	value := getEnvBool("TEST_BOOL", false)
	if value != true {
		t.Errorf("expected true, got %v", value)
	}

	value = getEnvBool("NONEXISTENT", false)
	if value != false {
		t.Errorf("expected false, got %v", value)
	}
}

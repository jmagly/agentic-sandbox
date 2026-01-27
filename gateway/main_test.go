package main

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"
)

// TestLoadConfig tests configuration loading and validation
func TestLoadConfig(t *testing.T) {
	tests := []struct {
		name      string
		content   string
		wantError bool
	}{
		{
			name: "valid config",
			content: `
listen: ":8080"
routes:
  - path_prefix: "/api"
    upstream: "http://localhost:9000"
    auth_header: "Authorization"
    auth_value_env: "API_TOKEN"
rate_limit:
  requests_per_minute: 100
  burst: 20
audit:
  enabled: true
  log_path: "/tmp/gateway.log"
`,
			wantError: false,
		},
		{
			name:      "empty config",
			content:   "",
			wantError: true,
		},
		{
			name:      "invalid yaml",
			content:   "invalid: [unclosed",
			wantError: true,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tmpfile, err := os.CreateTemp("", "gateway-test-*.yaml")
			if err != nil {
				t.Fatal(err)
			}
			defer os.Remove(tmpfile.Name())

			if _, err := tmpfile.Write([]byte(tt.content)); err != nil {
				t.Fatal(err)
			}
			tmpfile.Close()

			_, err = loadConfig(tmpfile.Name())
			if (err != nil) != tt.wantError {
				t.Errorf("loadConfig() error = %v, wantError %v", err, tt.wantError)
			}
		})
	}
}

// TestRouteMatching tests path-based route matching
func TestRouteMatching(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:   "/api",
				Upstream:     "http://backend:9000",
				AuthHeader:   "Authorization",
				AuthValueEnv: "API_TOKEN",
			},
			{
				PathPrefix:   "/mcp",
				Upstream:     "http://mcp:8000",
				AuthHeader:   "X-API-Key",
				AuthValueEnv: "MCP_TOKEN",
			},
		},
	}

	gw := NewGateway(config)

	tests := []struct {
		path      string
		wantRoute *Route
	}{
		{"/api/users", &config.Routes[0]},
		{"/api/v1/data", &config.Routes[0]},
		{"/mcp/status", &config.Routes[1]},
		{"/unknown", nil},
	}

	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			route := gw.findRoute(tt.path)
			if tt.wantRoute == nil && route != nil {
				t.Errorf("findRoute(%q) = %v, want nil", tt.path, route)
			}
			if tt.wantRoute != nil && route == nil {
				t.Errorf("findRoute(%q) = nil, want route", tt.path)
			}
			if tt.wantRoute != nil && route != nil && route.PathPrefix != tt.wantRoute.PathPrefix {
				t.Errorf("findRoute(%q) prefix = %q, want %q", tt.path, route.PathPrefix, tt.wantRoute.PathPrefix)
			}
		})
	}
}

// TestTokenInjection tests auth header injection
func TestTokenInjection(t *testing.T) {
	// Set up test upstream server that echoes headers
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		apiKey := r.Header.Get("X-API-Key")
		resp := map[string]string{
			"auth":    auth,
			"api_key": apiKey,
		}
		json.NewEncoder(w).Encode(resp)
	}))
	defer upstream.Close()

	// Set environment variables for tokens
	os.Setenv("TEST_TOKEN", "secret-token-123")
	os.Setenv("TEST_API_KEY", "api-key-456")
	defer os.Unsetenv("TEST_TOKEN")
	defer os.Unsetenv("TEST_API_KEY")

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:   "/auth",
				Upstream:     upstream.URL,
				AuthHeader:   "Authorization",
				AuthValueEnv: "TEST_TOKEN",
			},
			{
				PathPrefix:   "/apikey",
				Upstream:     upstream.URL,
				AuthHeader:   "X-API-Key",
				AuthValueEnv: "TEST_API_KEY",
			},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	tests := []struct {
		path       string
		wantAuth   string
		wantAPIKey string
	}{
		{"/auth/test", "secret-token-123", ""},
		{"/apikey/test", "", "api-key-456"},
	}

	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			resp, err := http.Get(server.URL + tt.path)
			if err != nil {
				t.Fatal(err)
			}
			defer resp.Body.Close()

			var result map[string]string
			if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
				t.Fatal(err)
			}

			if result["auth"] != tt.wantAuth {
				t.Errorf("Authorization = %q, want %q", result["auth"], tt.wantAuth)
			}
			if result["api_key"] != tt.wantAPIKey {
				t.Errorf("X-API-Key = %q, want %q", result["api_key"], tt.wantAPIKey)
			}
		})
	}
}

// TestRateLimiting tests per-client rate limiting
func TestRateLimiting(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix: "/api",
				Upstream:   upstream.URL,
			},
		},
		RateLimit: RateLimitConfig{
			RequestsPerMinute: 5,
			Burst:             2,
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	// Make requests up to burst limit
	for i := 0; i < config.RateLimit.Burst; i++ {
		resp, err := http.Get(server.URL + "/api/test")
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Close()

		if resp.StatusCode != http.StatusOK {
			t.Errorf("Request %d: status = %d, want %d", i, resp.StatusCode, http.StatusOK)
		}
	}

	// Next request should be rate limited
	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()

	if resp.StatusCode != http.StatusTooManyRequests {
		t.Errorf("Rate limit status = %d, want %d", resp.StatusCode, http.StatusTooManyRequests)
	}

	// Check for rate limit headers
	if resp.Header.Get("X-RateLimit-Limit") == "" {
		t.Error("Missing X-RateLimit-Limit header")
	}
	if resp.Header.Get("Retry-After") == "" {
		t.Error("Missing Retry-After header")
	}
}

// TestHealthEndpoint tests the health check endpoint
func TestHealthEndpoint(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000"},
			{PathPrefix: "/mcp", Upstream: "http://mcp:8000"},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/health")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		t.Errorf("Health check status = %d, want %d", resp.StatusCode, http.StatusOK)
	}

	var health HealthResponse
	if err := json.NewDecoder(resp.Body).Decode(&health); err != nil {
		t.Fatal(err)
	}

	if health.Status != "healthy" {
		t.Errorf("Health status = %q, want %q", health.Status, "healthy")
	}
	if health.RoutesLoaded != len(config.Routes) {
		t.Errorf("Routes loaded = %d, want %d", health.RoutesLoaded, len(config.Routes))
	}
}

// TestPathSanitization tests input validation
func TestPathSanitization(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000"},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	tests := []struct {
		path       string
		wantStatus int
	}{
		{"/api/users", http.StatusOK}, // Will get 502 but that's fine - not rejected
		{"/api/../etc/passwd", http.StatusBadRequest},
		{"/api/test\x00", http.StatusBadRequest},
	}

	for _, tt := range tests {
		t.Run(tt.path, func(t *testing.T) {
			resp, err := http.Get(server.URL + tt.path)
			if err != nil {
				// For null byte test, the HTTP client may reject it
				if tt.wantStatus == http.StatusBadRequest {
					return
				}
				t.Fatal(err)
			}
			defer resp.Body.Close()

			if tt.wantStatus == http.StatusOK {
				// We expect 502 because upstream doesn't exist, not 400
				if resp.StatusCode == http.StatusBadRequest {
					t.Errorf("Path %q: status = %d, should not be rejected", tt.path, resp.StatusCode)
				}
			} else if resp.StatusCode != tt.wantStatus {
				t.Errorf("Path %q: status = %d, want %d", tt.path, resp.StatusCode, tt.wantStatus)
			}
		})
	}
}

// TestUpstreamError tests error handling
func TestUpstreamError(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix: "/api",
				Upstream:   "http://nonexistent-host-12345:9000",
			},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusBadGateway {
		t.Errorf("Upstream error status = %d, want %d", resp.StatusCode, http.StatusBadGateway)
	}
}

// TestAuditLogging tests that requests are logged without token exposure
func TestAuditLogging(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	// Create temporary log file
	tmpfile, err := os.CreateTemp("", "gateway-audit-*.log")
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(tmpfile.Name())
	tmpfile.Close()

	os.Setenv("TEST_SECRET_TOKEN", "super-secret-token-must-not-appear")
	defer os.Unsetenv("TEST_SECRET_TOKEN")

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:   "/api",
				Upstream:     upstream.URL,
				AuthHeader:   "Authorization",
				AuthValueEnv: "TEST_SECRET_TOKEN",
			},
		},
		Audit: AuditConfig{
			Enabled: true,
			LogPath: tmpfile.Name(),
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	// Make a request
	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()

	// Give logger time to flush
	time.Sleep(100 * time.Millisecond)

	// Read log file
	logContent, err := os.ReadFile(tmpfile.Name())
	if err != nil {
		t.Fatal(err)
	}

	logStr := string(logContent)

	// Verify token is NOT in logs
	if strings.Contains(logStr, "super-secret-token-must-not-appear") {
		t.Error("SECURITY VIOLATION: Token found in audit log")
	}

	// Verify path IS in logs
	if !strings.Contains(logStr, "/api/test") {
		t.Error("Expected path not found in audit log")
	}
}

// TestGracefulShutdown tests server shutdown
func TestGracefulShutdown(t *testing.T) {
	config := &Config{
		Listen: ":0", // Random port
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000"},
		},
	}

	gw := NewGateway(config)
	server := &http.Server{
		Handler:      gw,
		ReadTimeout:  30 * time.Second,
		WriteTimeout: 30 * time.Second,
	}

	// Start server in background
	go server.ListenAndServe()

	// Give it time to start
	time.Sleep(100 * time.Millisecond)

	// Shutdown with timeout
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	if err := server.Shutdown(ctx); err != nil {
		t.Errorf("Shutdown error: %v", err)
	}
}

// TestNoTokenConfigured tests behavior when token env var is not set
func TestNoTokenConfigured(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth := r.Header.Get("Authorization")
		if auth == "" {
			w.WriteHeader(http.StatusUnauthorized)
		} else {
			w.WriteHeader(http.StatusOK)
		}
	}))
	defer upstream.Close()

	// Ensure token env var is NOT set
	os.Unsetenv("MISSING_TOKEN")

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:   "/api",
				Upstream:     upstream.URL,
				AuthHeader:   "Authorization",
				AuthValueEnv: "MISSING_TOKEN",
			},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	// Should get 503 because token is not configured
	if resp.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("No token status = %d, want %d", resp.StatusCode, http.StatusServiceUnavailable)
	}
}

// TestRequestMethodsSupported tests that all HTTP methods work
func TestRequestMethodsSupported(t *testing.T) {
	methods := []string{"GET", "POST", "PUT", "DELETE", "PATCH"}

	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Method", r.Method)
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: upstream.URL},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	for _, method := range methods {
		t.Run(method, func(t *testing.T) {
			req, err := http.NewRequest(method, server.URL+"/api/test", bytes.NewReader([]byte("{}")))
			if err != nil {
				t.Fatal(err)
			}

			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			defer resp.Body.Close()

			if resp.StatusCode != http.StatusOK {
				t.Errorf("Method %s: status = %d, want %d", method, resp.StatusCode, http.StatusOK)
			}

			if resp.Header.Get("X-Method") != method {
				t.Errorf("Method %s: upstream received %q", method, resp.Header.Get("X-Method"))
			}
		})
	}
}

// TestRequestBodyProxied tests that request bodies are forwarded
func TestRequestBodyProxied(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Write(body)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: upstream.URL},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	testBody := `{"test": "data"}`
	resp, err := http.Post(server.URL+"/api/test", "application/json", strings.NewReader(testBody))
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		t.Fatal(err)
	}

	if string(body) != testBody {
		t.Errorf("Response body = %q, want %q", string(body), testBody)
	}
}

// TestCloseAuditFile tests that audit file is closed properly
func TestCloseAuditFile(t *testing.T) {
	tmpfile, err := os.CreateTemp("", "gateway-close-*.log")
	if err != nil {
		t.Fatal(err)
	}
	tmpfile.Close()
	defer os.Remove(tmpfile.Name())

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000"},
		},
		Audit: AuditConfig{
			Enabled: true,
			LogPath: tmpfile.Name(),
		},
	}

	gw := NewGateway(config)
	if err := gw.Close(); err != nil {
		t.Errorf("Close() error = %v", err)
	}
}

// TestCloseNoAuditFile tests Close() when no audit file is open
func TestCloseNoAuditFile(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000"},
		},
	}

	gw := NewGateway(config)
	if err := gw.Close(); err != nil {
		t.Errorf("Close() with no audit file error = %v", err)
	}
}

// TestLoadConfigNoRoutes tests that config without routes fails
func TestLoadConfigNoRoutes(t *testing.T) {
	tmpfile, err := os.CreateTemp("", "gateway-test-*.yaml")
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(tmpfile.Name())

	content := `
listen: ":8080"
routes: []
`
	if _, err := tmpfile.Write([]byte(content)); err != nil {
		t.Fatal(err)
	}
	tmpfile.Close()

	_, err = loadConfig(tmpfile.Name())
	if err == nil {
		t.Error("loadConfig() should fail with no routes")
	}
	if !strings.Contains(err.Error(), "no routes") {
		t.Errorf("loadConfig() error = %v, want 'no routes' error", err)
	}
}

// TestLoadConfigDefaults tests default values
func TestLoadConfigDefaults(t *testing.T) {
	tmpfile, err := os.CreateTemp("", "gateway-test-*.yaml")
	if err != nil {
		t.Fatal(err)
	}
	defer os.Remove(tmpfile.Name())

	content := `
routes:
  - path_prefix: "/api"
    upstream: "http://backend:9000"
`
	if _, err := tmpfile.Write([]byte(content)); err != nil {
		t.Fatal(err)
	}
	tmpfile.Close()

	config, err := loadConfig(tmpfile.Name())
	if err != nil {
		t.Fatal(err)
	}

	if config.Listen != ":8080" {
		t.Errorf("Default listen = %q, want %q", config.Listen, ":8080")
	}
	if config.DefaultAction != "deny" {
		t.Errorf("Default action = %q, want %q", config.DefaultAction, "deny")
	}
}

// TestValidatePathNullByte tests null byte validation
func TestValidatePathNullByte(t *testing.T) {
	err := validatePath("/api/test\x00")
	if err == nil {
		t.Error("validatePath() should reject null byte")
	}
	if !strings.Contains(err.Error(), "null byte") {
		t.Errorf("validatePath() error = %v, want 'null byte' error", err)
	}
}

// TestTokenValidationAtStartup tests token validation during initialization
func TestTokenValidationAtStartup(t *testing.T) {
	// Set a short token that should trigger warning
	os.Setenv("SHORT_TOKEN", "abc")
	defer os.Unsetenv("SHORT_TOKEN")

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:   "/api",
				Upstream:     "http://backend:9000",
				AuthValueEnv: "SHORT_TOKEN",
			},
		},
	}

	// Should initialize but log warning (we just verify no panic)
	gw := NewGateway(config)
	if gw == nil {
		t.Error("NewGateway() should not fail with short token")
	}
}

// TestInvalidUpstreamURL tests behavior with invalid upstream URL in route
func TestInvalidUpstreamURL(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix: "/api",
				Upstream:   "://invalid-url",
			},
		},
	}

	// Should initialize but log warning
	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	// Should get 500 error for invalid upstream
	if resp.StatusCode != http.StatusInternalServerError {
		t.Errorf("Invalid upstream status = %d, want %d", resp.StatusCode, http.StatusInternalServerError)
	}
}

// TestHealthEndpointDetails tests health check response details
func TestHealthEndpointDetails(t *testing.T) {
	os.Setenv("HEALTH_TOKEN", "test-token-for-health")
	defer os.Unsetenv("HEALTH_TOKEN")

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000", AuthValueEnv: "HEALTH_TOKEN"},
			{PathPrefix: "/public", Upstream: "http://public:9000"},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	time.Sleep(100 * time.Millisecond) // Let some uptime accumulate

	resp, err := http.Get(server.URL + "/health")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	var health HealthResponse
	if err := json.NewDecoder(resp.Body).Decode(&health); err != nil {
		t.Fatal(err)
	}

	if health.Status != "healthy" {
		t.Errorf("Health status = %q, want %q", health.Status, "healthy")
	}
	if health.RoutesLoaded != 2 {
		t.Errorf("Routes loaded = %d, want 2", health.RoutesLoaded)
	}
	if health.TokensLoaded != 1 {
		t.Errorf("Tokens loaded = %d, want 1", health.TokensLoaded)
	}
	if health.UptimeSeconds < 0 {
		t.Errorf("Uptime = %d, should be >= 0", health.UptimeSeconds)
	}
}

// TestProxyRequestWithStripPrefix tests path stripping
func TestProxyRequestWithStripPrefix(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Echo the path we received
		w.Header().Set("X-Received-Path", r.URL.Path)
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:  "/api",
				Upstream:    upstream.URL,
				StripPrefix: true,
			},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/users/123")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	receivedPath := resp.Header.Get("X-Received-Path")
	expectedPath := "/users/123"
	if receivedPath != expectedPath {
		t.Errorf("Stripped path = %q, want %q", receivedPath, expectedPath)
	}
}

// TestProxyRequestWithoutStripPrefix tests path forwarding without stripping
func TestProxyRequestWithoutStripPrefix(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-Received-Path", r.URL.Path)
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:  "/api",
				Upstream:    upstream.URL,
				StripPrefix: false,
			},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/users")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	receivedPath := resp.Header.Get("X-Received-Path")
	expectedPath := "/api/users"
	if receivedPath != expectedPath {
		t.Errorf("Forwarded path = %q, want %q", receivedPath, expectedPath)
	}
}

// TestUserAgentHeader tests that gateway sets User-Agent
func TestUserAgentHeader(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("X-User-Agent", r.Header.Get("User-Agent"))
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: upstream.URL},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	userAgent := resp.Header.Get("X-User-Agent")
	if !strings.Contains(userAgent, "agentic-sandbox-gateway") {
		t.Errorf("User-Agent = %q, should contain 'agentic-sandbox-gateway'", userAgent)
	}
}

// TestRateLimitDefaultBurst tests default burst calculation
func TestRateLimitDefaultBurst(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: upstream.URL},
		},
		RateLimit: RateLimitConfig{
			RequestsPerMinute: 100,
			// No burst specified - should use default
		},
	}

	gw := NewGateway(config)
	if gw.rateLimiter == nil {
		t.Error("Rate limiter should be initialized")
	}
}

// TestAuditLogFailure tests handling when audit log can't be opened
func TestAuditLogFailure(t *testing.T) {
	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{PathPrefix: "/api", Upstream: "http://backend:9000"},
		},
		Audit: AuditConfig{
			Enabled: true,
			LogPath: "/nonexistent/directory/audit.log",
		},
	}

	// Should not panic, just log warning
	gw := NewGateway(config)
	if gw == nil {
		t.Error("NewGateway() should not fail even if audit log fails to open")
	}
}

// TestCustomAuthHeader tests custom auth header name
func TestCustomAuthHeader(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		customHeader := r.Header.Get("X-Custom-Auth")
		w.Header().Set("X-Auth-Received", customHeader)
		w.WriteHeader(http.StatusOK)
	}))
	defer upstream.Close()

	os.Setenv("CUSTOM_TOKEN", "custom-secret-123")
	defer os.Unsetenv("CUSTOM_TOKEN")

	config := &Config{
		Listen: ":8080",
		Routes: []Route{
			{
				PathPrefix:   "/api",
				Upstream:     upstream.URL,
				AuthHeader:   "X-Custom-Auth",
				AuthValueEnv: "CUSTOM_TOKEN",
			},
		},
	}

	gw := NewGateway(config)
	server := httptest.NewServer(gw)
	defer server.Close()

	resp, err := http.Get(server.URL + "/api/test")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()

	authReceived := resp.Header.Get("X-Auth-Received")
	if authReceived != "custom-secret-123" {
		t.Errorf("Custom auth header = %q, want %q", authReceived, "custom-secret-123")
	}
}

package api

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"github.com/roctinam/agentic-sandbox/internal/config"
	"github.com/roctinam/agentic-sandbox/internal/sandbox"
)

func TestNewServer(t *testing.T) {
	cfg := &config.Config{
		Server: config.ServerConfig{
			Host: "localhost",
			Port: 8080,
		},
	}
	mgr := sandbox.NewManager()

	server := NewServer(cfg, mgr)

	if server == nil {
		t.Fatal("expected server to be non-nil")
	}
	if server.router == nil {
		t.Fatal("expected router to be initialized")
	}
	if server.handlers == nil {
		t.Fatal("expected handlers to be initialized")
	}
}

func TestServerRoutes(t *testing.T) {
	cfg := &config.Config{
		Server: config.ServerConfig{
			Host: "localhost",
			Port: 8080,
		},
	}
	mgr := sandbox.NewManager()
	server := NewServer(cfg, mgr)

	tests := []struct {
		name       string
		method     string
		path       string
		wantStatus int
	}{
		{
			name:       "health check",
			method:     "GET",
			path:       "/health",
			wantStatus: http.StatusOK,
		},
		{
			name:       "list sandboxes",
			method:     "GET",
			path:       "/api/v1/sandboxes",
			wantStatus: http.StatusOK,
		},
		{
			name:       "get nonexistent sandbox",
			method:     "GET",
			path:       "/api/v1/sandboxes/nonexistent",
			wantStatus: http.StatusNotFound,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			req := httptest.NewRequest(tt.method, tt.path, nil)
			rr := httptest.NewRecorder()

			server.router.ServeHTTP(rr, req)

			if rr.Code != tt.wantStatus {
				t.Errorf("expected status %d, got %d", tt.wantStatus, rr.Code)
			}
		})
	}
}

func TestServerShutdown(t *testing.T) {
	cfg := &config.Config{
		Server: config.ServerConfig{
			Host: "localhost",
			Port: 8080,
		},
	}
	mgr := sandbox.NewManager()
	server := NewServer(cfg, mgr)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	// Shutdown without starting should not error
	if err := server.Shutdown(ctx); err != nil {
		t.Errorf("expected no error on shutdown, got %v", err)
	}
}

func TestServerMiddleware(t *testing.T) {
	cfg := &config.Config{
		Server: config.ServerConfig{
			Host: "localhost",
			Port: 8080,
		},
	}
	mgr := sandbox.NewManager()
	server := NewServer(cfg, mgr)

	req := httptest.NewRequest("GET", "/health", nil)
	rr := httptest.NewRecorder()

	server.router.ServeHTTP(rr, req)

	// Check CORS headers
	if rr.Header().Get("Access-Control-Allow-Origin") != "*" {
		t.Error("expected CORS middleware to be applied")
	}

	// Check that request completed successfully (logging middleware didn't break it)
	if rr.Code != http.StatusOK {
		t.Errorf("expected status 200, got %d", rr.Code)
	}
}

package api

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/go-chi/chi/v5"
	"github.com/roctinam/agentic-sandbox/internal/sandbox"
)

func TestHealthCheck(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	req := httptest.NewRequest("GET", "/health", nil)
	rr := httptest.NewRecorder()

	handlers.HealthCheck(rr, req)

	if rr.Code != http.StatusOK {
		t.Errorf("expected status 200, got %d", rr.Code)
	}

	var response map[string]string
	if err := json.NewDecoder(rr.Body).Decode(&response); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if response["status"] != "healthy" {
		t.Errorf("expected status 'healthy', got '%s'", response["status"])
	}
}

func TestCreateSandbox(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	spec := sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
	}

	body, _ := json.Marshal(spec)
	req := httptest.NewRequest("POST", "/sandboxes", bytes.NewReader(body))
	rr := httptest.NewRecorder()

	handlers.CreateSandbox(rr, req)

	if rr.Code != http.StatusCreated {
		t.Errorf("expected status 201, got %d", rr.Code)
	}

	var created sandbox.Sandbox
	if err := json.NewDecoder(rr.Body).Decode(&created); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if created.Name != "test" {
		t.Errorf("expected name 'test', got '%s'", created.Name)
	}
}

func TestCreateSandboxInvalidBody(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	req := httptest.NewRequest("POST", "/sandboxes", bytes.NewReader([]byte("invalid")))
	rr := httptest.NewRecorder()

	handlers.CreateSandbox(rr, req)

	if rr.Code != http.StatusBadRequest {
		t.Errorf("expected status 400 for invalid body, got %d", rr.Code)
	}
}

func TestGetSandbox(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	// Create a sandbox first
	spec := &sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
	}
	created, _ := mgr.Create(httptest.NewRequest("GET", "/", nil).Context(), spec)

	// Set up router to extract path parameter
	r := chi.NewRouter()
	r.Get("/sandboxes/{id}", handlers.GetSandbox)

	req := httptest.NewRequest("GET", "/sandboxes/"+created.ID, nil)
	rr := httptest.NewRecorder()

	r.ServeHTTP(rr, req)

	if rr.Code != http.StatusOK {
		t.Errorf("expected status 200, got %d", rr.Code)
	}

	var retrieved sandbox.Sandbox
	if err := json.NewDecoder(rr.Body).Decode(&retrieved); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if retrieved.ID != created.ID {
		t.Errorf("expected ID '%s', got '%s'", created.ID, retrieved.ID)
	}
}

func TestGetSandboxNotFound(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	r := chi.NewRouter()
	r.Get("/sandboxes/{id}", handlers.GetSandbox)

	req := httptest.NewRequest("GET", "/sandboxes/nonexistent", nil)
	rr := httptest.NewRecorder()

	r.ServeHTTP(rr, req)

	if rr.Code != http.StatusNotFound {
		t.Errorf("expected status 404, got %d", rr.Code)
	}
}

func TestListSandboxes(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	// Create a sandbox
	spec := &sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
	}
	mgr.Create(httptest.NewRequest("GET", "/", nil).Context(), spec)

	req := httptest.NewRequest("GET", "/sandboxes", nil)
	rr := httptest.NewRecorder()

	handlers.ListSandboxes(rr, req)

	if rr.Code != http.StatusOK {
		t.Errorf("expected status 200, got %d", rr.Code)
	}

	var sandboxes []*sandbox.Sandbox
	if err := json.NewDecoder(rr.Body).Decode(&sandboxes); err != nil {
		t.Fatalf("failed to decode response: %v", err)
	}

	if len(sandboxes) != 1 {
		t.Errorf("expected 1 sandbox, got %d", len(sandboxes))
	}
}

func TestStartSandbox(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	spec := &sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
	}
	created, _ := mgr.Create(httptest.NewRequest("GET", "/", nil).Context(), spec)

	r := chi.NewRouter()
	r.Post("/sandboxes/{id}/start", handlers.StartSandbox)

	req := httptest.NewRequest("POST", "/sandboxes/"+created.ID+"/start", nil)
	rr := httptest.NewRecorder()

	r.ServeHTTP(rr, req)

	if rr.Code != http.StatusNoContent {
		t.Errorf("expected status 204, got %d", rr.Code)
	}
}

func TestStopSandbox(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	spec := &sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
		AutoStart: true,
	}
	created, _ := mgr.Create(httptest.NewRequest("GET", "/", nil).Context(), spec)

	r := chi.NewRouter()
	r.Post("/sandboxes/{id}/stop", handlers.StopSandbox)

	req := httptest.NewRequest("POST", "/sandboxes/"+created.ID+"/stop", nil)
	rr := httptest.NewRecorder()

	r.ServeHTTP(rr, req)

	if rr.Code != http.StatusNoContent {
		t.Errorf("expected status 204, got %d", rr.Code)
	}
}

func TestDeleteSandbox(t *testing.T) {
	mgr := sandbox.NewManager()
	handlers := NewHandlers(mgr)

	spec := &sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
	}
	created, _ := mgr.Create(httptest.NewRequest("GET", "/", nil).Context(), spec)

	r := chi.NewRouter()
	r.Delete("/sandboxes/{id}", handlers.DeleteSandbox)

	req := httptest.NewRequest("DELETE", "/sandboxes/"+created.ID, nil)
	rr := httptest.NewRecorder()

	r.ServeHTTP(rr, req)

	if rr.Code != http.StatusNoContent {
		t.Errorf("expected status 204, got %d", rr.Code)
	}
}

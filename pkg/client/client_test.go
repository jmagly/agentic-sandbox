package client

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/roctinam/agentic-sandbox/internal/sandbox"
)

func TestNewClient(t *testing.T) {
	client := NewClient("http://localhost:8080")

	if client == nil {
		t.Fatal("expected client to be non-nil")
	}
	if client.baseURL != "http://localhost:8080" {
		t.Errorf("expected baseURL 'http://localhost:8080', got '%s'", client.baseURL)
	}
	if client.httpClient == nil {
		t.Fatal("expected httpClient to be initialized")
	}
}

func TestHealth(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/health" {
			t.Errorf("expected path '/health', got '%s'", r.URL.Path)
		}
		w.WriteHeader(http.StatusOK)
		json.NewEncoder(w).Encode(map[string]string{"status": "healthy"})
	}))
	defer server.Close()

	client := NewClient(server.URL)
	err := client.Health(context.Background())

	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}

func TestCreateSandbox(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			t.Errorf("expected POST, got %s", r.Method)
		}
		if r.URL.Path != "/api/v1/sandboxes" {
			t.Errorf("expected path '/api/v1/sandboxes', got '%s'", r.URL.Path)
		}

		var spec sandbox.SandboxSpec
		json.NewDecoder(r.Body).Decode(&spec)

		sb := &sandbox.Sandbox{
			ID:      "test-123",
			Name:    spec.Name,
			Runtime: spec.Runtime,
			Image:   spec.Image,
			State:   sandbox.StateCreated,
		}

		w.WriteHeader(http.StatusCreated)
		json.NewEncoder(w).Encode(sb)
	}))
	defer server.Close()

	client := NewClient(server.URL)
	spec := &sandbox.SandboxSpec{
		Name:      "test",
		Runtime:   "docker",
		Image:     "test:latest",
		Resources: sandbox.DefaultResources(),
	}

	sb, err := client.CreateSandbox(context.Background(), spec)

	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if sb.ID != "test-123" {
		t.Errorf("expected ID 'test-123', got '%s'", sb.ID)
	}
}

func TestGetSandbox(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "GET" {
			t.Errorf("expected GET, got %s", r.Method)
		}

		sb := &sandbox.Sandbox{
			ID:      "test-123",
			Name:    "test",
			Runtime: "docker",
			Image:   "test:latest",
			State:   sandbox.StateRunning,
		}

		w.WriteHeader(http.StatusOK)
		json.NewEncoder(w).Encode(sb)
	}))
	defer server.Close()

	client := NewClient(server.URL)
	sb, err := client.GetSandbox(context.Background(), "test-123")

	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if sb.ID != "test-123" {
		t.Errorf("expected ID 'test-123', got '%s'", sb.ID)
	}
}

func TestListSandboxes(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		sandboxes := []*sandbox.Sandbox{
			{ID: "test-1", Name: "test1"},
			{ID: "test-2", Name: "test2"},
		}

		w.WriteHeader(http.StatusOK)
		json.NewEncoder(w).Encode(sandboxes)
	}))
	defer server.Close()

	client := NewClient(server.URL)
	sandboxes, err := client.ListSandboxes(context.Background())

	if err != nil {
		t.Fatalf("expected no error, got %v", err)
	}
	if len(sandboxes) != 2 {
		t.Errorf("expected 2 sandboxes, got %d", len(sandboxes))
	}
}

func TestStartSandbox(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			t.Errorf("expected POST, got %s", r.Method)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	defer server.Close()

	client := NewClient(server.URL)
	err := client.StartSandbox(context.Background(), "test-123")

	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}

func TestStopSandbox(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			t.Errorf("expected POST, got %s", r.Method)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	defer server.Close()

	client := NewClient(server.URL)
	err := client.StopSandbox(context.Background(), "test-123")

	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}

func TestDeleteSandbox(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "DELETE" {
			t.Errorf("expected DELETE, got %s", r.Method)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	defer server.Close()

	client := NewClient(server.URL)
	err := client.DeleteSandbox(context.Background(), "test-123")

	if err != nil {
		t.Errorf("expected no error, got %v", err)
	}
}

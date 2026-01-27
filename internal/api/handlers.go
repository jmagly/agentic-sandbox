package api

import (
	"encoding/json"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/roctinam/agentic-sandbox/internal/sandbox"
	"github.com/rs/zerolog/log"
)

// Handlers holds HTTP request handlers
type Handlers struct {
	manager *sandbox.Manager
}

// NewHandlers creates a new handlers instance
func NewHandlers(manager *sandbox.Manager) *Handlers {
	return &Handlers{
		manager: manager,
	}
}

// HealthCheck handles health check requests
func (h *Handlers) HealthCheck(w http.ResponseWriter, r *http.Request) {
	response := map[string]string{
		"status": "healthy",
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(response)
}

// CreateSandbox handles sandbox creation requests
func (h *Handlers) CreateSandbox(w http.ResponseWriter, r *http.Request) {
	var spec sandbox.SandboxSpec

	if err := json.NewDecoder(r.Body).Decode(&spec); err != nil {
		log.Error().Err(err).Msg("failed to decode sandbox spec")
		http.Error(w, "invalid request body", http.StatusBadRequest)
		return
	}

	sb, err := h.manager.Create(r.Context(), &spec)
	if err != nil {
		log.Error().Err(err).Msg("failed to create sandbox")
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	json.NewEncoder(w).Encode(sb)
}

// GetSandbox handles sandbox retrieval requests
func (h *Handlers) GetSandbox(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")

	sb, err := h.manager.Get(r.Context(), id)
	if err != nil {
		log.Error().Err(err).Str("id", id).Msg("failed to get sandbox")
		http.Error(w, err.Error(), http.StatusNotFound)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(sb)
}

// ListSandboxes handles sandbox listing requests
func (h *Handlers) ListSandboxes(w http.ResponseWriter, r *http.Request) {
	sandboxes, err := h.manager.List(r.Context())
	if err != nil {
		log.Error().Err(err).Msg("failed to list sandboxes")
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	json.NewEncoder(w).Encode(sandboxes)
}

// StartSandbox handles sandbox start requests
func (h *Handlers) StartSandbox(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")

	if err := h.manager.Start(r.Context(), id); err != nil {
		log.Error().Err(err).Str("id", id).Msg("failed to start sandbox")
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(http.StatusNoContent)
}

// StopSandbox handles sandbox stop requests
func (h *Handlers) StopSandbox(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")

	if err := h.manager.Stop(r.Context(), id); err != nil {
		log.Error().Err(err).Str("id", id).Msg("failed to stop sandbox")
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(http.StatusNoContent)
}

// DeleteSandbox handles sandbox deletion requests
func (h *Handlers) DeleteSandbox(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")

	if err := h.manager.Delete(r.Context(), id); err != nil {
		log.Error().Err(err).Str("id", id).Msg("failed to delete sandbox")
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}

	w.WriteHeader(http.StatusNoContent)
}

// TODO: Add handlers for:
// - ExecSandbox - Execute command in sandbox
// - GetSandboxLogs - Retrieve sandbox logs
// - GetSandboxStats - Get resource usage stats

package api

import (
	"context"
	"fmt"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/roctinam/agentic-sandbox/internal/config"
	"github.com/roctinam/agentic-sandbox/internal/sandbox"
	"github.com/rs/zerolog/log"
)

// Server represents the HTTP API server
type Server struct {
	config   *config.Config
	manager  *sandbox.Manager
	handlers *Handlers
	router   *chi.Mux
	server   *http.Server
}

// NewServer creates a new API server
func NewServer(cfg *config.Config, manager *sandbox.Manager) *Server {
	handlers := NewHandlers(manager)
	router := chi.NewRouter()

	s := &Server{
		config:   cfg,
		manager:  manager,
		handlers: handlers,
		router:   router,
	}

	s.setupRoutes()

	return s
}

// setupRoutes configures HTTP routes
func (s *Server) setupRoutes() {
	// Middleware
	s.router.Use(RecoveryMiddleware)
	s.router.Use(LoggingMiddleware)
	s.router.Use(CORSMiddleware)

	// Health check
	s.router.Get("/health", s.handlers.HealthCheck)

	// API v1
	s.router.Route("/api/v1", func(r chi.Router) {
		// Sandboxes
		r.Route("/sandboxes", func(r chi.Router) {
			r.Get("/", s.handlers.ListSandboxes)
			r.Post("/", s.handlers.CreateSandbox)

			r.Route("/{id}", func(r chi.Router) {
				r.Get("/", s.handlers.GetSandbox)
				r.Delete("/", s.handlers.DeleteSandbox)
				r.Post("/start", s.handlers.StartSandbox)
				r.Post("/stop", s.handlers.StopSandbox)
				// TODO: Add exec, logs, stats endpoints
			})
		})
	})
}

// Start starts the HTTP server
func (s *Server) Start() error {
	addr := fmt.Sprintf("%s:%d", s.config.Server.Host, s.config.Server.Port)

	s.server = &http.Server{
		Addr:         addr,
		Handler:      s.router,
		ReadTimeout:  15 * time.Second,
		WriteTimeout: 15 * time.Second,
		IdleTimeout:  60 * time.Second,
	}

	log.Info().
		Str("address", addr).
		Msg("starting HTTP server")

	if err := s.server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		return fmt.Errorf("server failed: %w", err)
	}

	return nil
}

// Shutdown gracefully shuts down the server
func (s *Server) Shutdown(ctx context.Context) error {
	log.Info().Msg("shutting down HTTP server")

	if s.server != nil {
		return s.server.Shutdown(ctx)
	}

	return nil
}

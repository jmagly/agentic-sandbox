package main

import (
	"context"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/roctinam/agentic-sandbox/internal/api"
	"github.com/roctinam/agentic-sandbox/internal/config"
	"github.com/roctinam/agentic-sandbox/internal/sandbox"
	"github.com/rs/zerolog"
	"github.com/rs/zerolog/log"
)

func main() {
	// Configure logging
	zerolog.TimeFieldFormat = zerolog.TimeFormatUnix
	log.Logger = log.Output(zerolog.ConsoleWriter{Out: os.Stderr})

	log.Info().Msg("starting agentic sandbox manager")

	// Load configuration
	cfg, err := config.Load()
	if err != nil {
		log.Fatal().Err(err).Msg("failed to load configuration")
	}

	if err := cfg.Validate(); err != nil {
		log.Fatal().Err(err).Msg("invalid configuration")
	}

	log.Info().
		Str("host", cfg.Server.Host).
		Int("port", cfg.Server.Port).
		Bool("seccomp", cfg.Security.EnableSeccomp).
		Msg("configuration loaded")

	// Create sandbox manager
	manager := sandbox.NewManager()

	// Create and start API server
	server := api.NewServer(cfg, manager)

	// Start server in goroutine
	go func() {
		if err := server.Start(); err != nil {
			log.Fatal().Err(err).Msg("server failed to start")
		}
	}()

	// Wait for interrupt signal
	quit := make(chan os.Signal, 1)
	signal.Notify(quit, syscall.SIGINT, syscall.SIGTERM)
	<-quit

	log.Info().Msg("shutting down server")

	// Graceful shutdown
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	if err := server.Shutdown(ctx); err != nil {
		log.Error().Err(err).Msg("server shutdown failed")
	}

	log.Info().Msg("server stopped")
}

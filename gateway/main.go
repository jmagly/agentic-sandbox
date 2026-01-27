// Auth injection gateway for agentic sandbox
// Adds authentication tokens to requests in-flight
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"
	"time"

	"golang.org/x/time/rate"
	"gopkg.in/yaml.v3"
)

// Config represents the gateway configuration
type Config struct {
	Listen        string           `yaml:"listen"`
	Routes        []Route          `yaml:"routes"`
	RateLimit     RateLimitConfig  `yaml:"rate_limit"`
	Audit         AuditConfig      `yaml:"audit"`
	DefaultAction string           `yaml:"default_action"`
}

// Route represents a routing rule
type Route struct {
	PathPrefix   string `yaml:"path_prefix"`
	Upstream     string `yaml:"upstream"`
	AuthHeader   string `yaml:"auth_header"`
	AuthValueEnv string `yaml:"auth_value_env"`
	StripPrefix  bool   `yaml:"strip_prefix"`
}

// RateLimitConfig configures rate limiting
type RateLimitConfig struct {
	RequestsPerMinute int `yaml:"requests_per_minute"`
	Burst             int `yaml:"burst"`
}

// AuditConfig configures audit logging
type AuditConfig struct {
	Enabled bool   `yaml:"enabled"`
	LogPath string `yaml:"log_path"`
}

// HealthResponse is the health check response
type HealthResponse struct {
	Status        string `json:"status"`
	UptimeSeconds int64  `json:"uptime_seconds"`
	RoutesLoaded  int    `json:"routes_loaded"`
	TokensLoaded  int    `json:"tokens_loaded"`
}

// Gateway is the main gateway handler
type Gateway struct {
	config      *Config
	startTime   time.Time
	rateLimiter *rate.Limiter
	auditLogger *log.Logger
	auditFile   *os.File
	mu          sync.RWMutex
}

// NewGateway creates a new gateway instance
func NewGateway(config *Config) *Gateway {
	gw := &Gateway{
		config:    config,
		startTime: time.Now(),
	}

	// Initialize rate limiter if configured
	if config.RateLimit.RequestsPerMinute > 0 {
		r := rate.Limit(float64(config.RateLimit.RequestsPerMinute) / 60.0) // per second
		burst := config.RateLimit.Burst
		if burst == 0 {
			burst = config.RateLimit.RequestsPerMinute / 10 // Default to 10% of per-minute
		}
		gw.rateLimiter = rate.NewLimiter(r, burst)
	}

	// Initialize audit logging if enabled
	if config.Audit.Enabled && config.Audit.LogPath != "" {
		f, err := os.OpenFile(config.Audit.LogPath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0600)
		if err != nil {
			log.Printf("WARNING: Failed to open audit log: %v", err)
		} else {
			gw.auditFile = f
			gw.auditLogger = log.New(f, "", 0)
		}
	}

	// Validate tokens at startup
	tokensLoaded := 0
	for i, route := range config.Routes {
		if route.AuthValueEnv != "" {
			token := os.Getenv(route.AuthValueEnv)
			if token == "" {
				log.Printf("WARNING: Token %s not set for route %s", route.AuthValueEnv, route.PathPrefix)
			} else if len(token) < 10 {
				log.Printf("WARNING: Token %s appears too short for route %s", route.AuthValueEnv, route.PathPrefix)
			} else {
				log.Printf("Loaded token from %s for route %s", route.AuthValueEnv, route.PathPrefix)
				tokensLoaded++
			}
		}

		// Validate upstream URL
		if _, err := url.Parse(route.Upstream); err != nil {
			log.Printf("WARNING: Invalid upstream URL for route %d: %v", i, err)
		}
	}

	log.Printf("Gateway initialized: %d routes, %d tokens loaded", len(config.Routes), tokensLoaded)
	return gw
}

// Close cleans up gateway resources
func (g *Gateway) Close() error {
	if g.auditFile != nil {
		return g.auditFile.Close()
	}
	return nil
}

// ServeHTTP handles incoming HTTP requests
func (g *Gateway) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	start := time.Now()

	// Health check endpoint
	if r.URL.Path == "/health" {
		g.handleHealth(w, r)
		return
	}

	// Validate path for security
	if err := validatePath(r.URL.Path); err != nil {
		g.logRequest(r, 0, start, "REJECTED", err.Error())
		http.Error(w, "Invalid request path", http.StatusBadRequest)
		return
	}

	// Rate limiting
	if g.rateLimiter != nil {
		if !g.rateLimiter.Allow() {
			w.Header().Set("X-RateLimit-Limit", fmt.Sprintf("%d", g.config.RateLimit.RequestsPerMinute))
			w.Header().Set("X-RateLimit-Remaining", "0")
			w.Header().Set("Retry-After", "60")

			g.logRequest(r, http.StatusTooManyRequests, start, "RATE_LIMITED", "")
			http.Error(w, `{"error": "rate_limit_exceeded"}`, http.StatusTooManyRequests)
			return
		}
	}

	// Find matching route
	route := g.findRoute(r.URL.Path)
	if route == nil {
		g.logRequest(r, http.StatusForbidden, start, "NO_ROUTE", "")
		http.Error(w, `{"error": "route_not_found"}`, http.StatusForbidden)
		return
	}

	// Check if token is configured (if required)
	if route.AuthValueEnv != "" {
		token := os.Getenv(route.AuthValueEnv)
		if token == "" {
			g.logRequest(r, http.StatusServiceUnavailable, start, "NO_TOKEN", route.AuthValueEnv)
			http.Error(w, `{"error": "token_not_configured"}`, http.StatusServiceUnavailable)
			return
		}
	}

	// Proxy the request
	g.proxyRequest(w, r, route, start)
}

// findRoute finds the matching route for a path
func (g *Gateway) findRoute(path string) *Route {
	for i := range g.config.Routes {
		if strings.HasPrefix(path, g.config.Routes[i].PathPrefix) {
			return &g.config.Routes[i]
		}
	}
	return nil
}

// validatePath validates the request path for security issues
func validatePath(path string) error {
	// Check for null bytes
	if strings.Contains(path, "\x00") {
		return fmt.Errorf("null byte in path")
	}

	// Check for path traversal
	if strings.Contains(path, "..") {
		return fmt.Errorf("path traversal attempt")
	}

	return nil
}

// proxyRequest proxies the request to the upstream server
func (g *Gateway) proxyRequest(w http.ResponseWriter, r *http.Request, route *Route, start time.Time) {
	// Parse upstream URL
	upstreamURL, err := url.Parse(route.Upstream)
	if err != nil {
		g.logRequest(r, http.StatusInternalServerError, start, "INVALID_UPSTREAM", err.Error())
		http.Error(w, `{"error": "invalid_upstream"}`, http.StatusInternalServerError)
		return
	}

	// Create reverse proxy
	proxy := httputil.NewSingleHostReverseProxy(upstreamURL)

	// Customize director to inject auth and modify path
	originalDirector := proxy.Director
	proxy.Director = func(req *http.Request) {
		originalDirector(req)

		// Inject auth header if configured
		if route.AuthValueEnv != "" {
			token := os.Getenv(route.AuthValueEnv)
			if token != "" {
				header := route.AuthHeader
				if header == "" {
					header = "Authorization"
				}
				req.Header.Set(header, token)
			}
		}

		// Strip prefix if configured
		if route.StripPrefix {
			req.URL.Path = strings.TrimPrefix(r.URL.Path, route.PathPrefix)
			if req.URL.Path == "" {
				req.URL.Path = "/"
			}
		}

		// Set user agent
		req.Header.Set("User-Agent", "agentic-sandbox-gateway/1.0")
	}

	// Custom error handler
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, err error) {
		g.logRequest(r, http.StatusBadGateway, start, "UPSTREAM_ERROR", err.Error())
		http.Error(w, `{"error": "upstream_error"}`, http.StatusBadGateway)
	}

	// Wrap response writer to capture status code
	rw := &responseWriter{ResponseWriter: w, statusCode: http.StatusOK}
	proxy.ServeHTTP(rw, r)

	// Log successful proxy
	g.logRequest(r, rw.statusCode, start, "PROXIED", route.Upstream)
}

// responseWriter wraps http.ResponseWriter to capture status code
type responseWriter struct {
	http.ResponseWriter
	statusCode int
}

func (rw *responseWriter) WriteHeader(code int) {
	rw.statusCode = code
	rw.ResponseWriter.WriteHeader(code)
}

// handleHealth handles health check requests
func (g *Gateway) handleHealth(w http.ResponseWriter, r *http.Request) {
	// Count tokens loaded
	tokensLoaded := 0
	for _, route := range g.config.Routes {
		if route.AuthValueEnv != "" {
			if os.Getenv(route.AuthValueEnv) != "" {
				tokensLoaded++
			}
		}
	}

	health := HealthResponse{
		Status:        "healthy",
		UptimeSeconds: int64(time.Since(g.startTime).Seconds()),
		RoutesLoaded:  len(g.config.Routes),
		TokensLoaded:  tokensLoaded,
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(health)
}

// logRequest logs request details (NEVER logs tokens)
func (g *Gateway) logRequest(r *http.Request, status int, start time.Time, event, detail string) {
	elapsed := time.Since(start)

	// Build log entry (structured JSON)
	logEntry := map[string]interface{}{
		"timestamp":  time.Now().UTC().Format(time.RFC3339),
		"level":      "INFO",
		"component":  "gateway",
		"event":      event,
		"method":     r.Method,
		"path":       r.URL.Path,
		"status":     status,
		"latency_ms": elapsed.Milliseconds(),
		"remote":     r.RemoteAddr,
	}

	if detail != "" && event != "NO_TOKEN" {
		// NEVER log token values, only env var names for NO_TOKEN
		logEntry["detail"] = detail
	}

	// Log to audit file if enabled
	if g.auditLogger != nil {
		logJSON, _ := json.Marshal(logEntry)
		g.auditLogger.Println(string(logJSON))
	}

	// Also log to stdout
	log.Printf("[%s] %s %s -> %d (%v)", event, r.Method, r.URL.Path, status, elapsed)
}

// loadConfig loads configuration from a YAML file
func loadConfig(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read config: %w", err)
	}

	if len(data) == 0 {
		return nil, fmt.Errorf("config file is empty")
	}

	var config Config
	if err := yaml.Unmarshal(data, &config); err != nil {
		return nil, fmt.Errorf("failed to parse config: %w", err)
	}

	// Set defaults
	if config.Listen == "" {
		config.Listen = ":8080"
	}
	if config.DefaultAction == "" {
		config.DefaultAction = "deny"
	}

	// Validate required fields
	if len(config.Routes) == 0 {
		return nil, fmt.Errorf("no routes configured")
	}

	return &config, nil
}

func main() {
	configPath := flag.String("config", "gateway.yaml", "Path to config file")
	flag.Parse()

	// Load configuration
	config, err := loadConfig(*configPath)
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	// Create gateway
	gw := NewGateway(config)
	defer gw.Close()

	// Create HTTP server
	server := &http.Server{
		Addr:         config.Listen,
		Handler:      gw,
		ReadTimeout:  30 * time.Second,
		WriteTimeout: 30 * time.Second,
		IdleTimeout:  120 * time.Second,
	}

	// Setup graceful shutdown
	stop := make(chan os.Signal, 1)
	signal.Notify(stop, os.Interrupt, syscall.SIGTERM)

	// Start server in background
	go func() {
		log.Printf("Auth Gateway starting on %s", config.Listen)
		log.Printf("Loaded %d routes", len(config.Routes))
		for _, r := range config.Routes {
			log.Printf("  %s -> %s", r.PathPrefix, r.Upstream)
		}

		if err := server.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			log.Fatalf("Server failed: %v", err)
		}
	}()

	// Wait for shutdown signal
	<-stop
	log.Println("Shutting down gracefully...")

	// Graceful shutdown with timeout
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	if err := server.Shutdown(ctx); err != nil {
		log.Printf("Shutdown error: %v", err)
	}

	log.Println("Gateway stopped")
}

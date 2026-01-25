// Auth injection gateway for agentic sandbox
// Adds authentication tokens to requests in-flight
package main

import (
	"flag"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"gopkg.in/yaml.v3"
)

type Route struct {
	Prefix   string `yaml:"prefix"`
	Upstream string `yaml:"upstream"`
	Auth     struct {
		Type     string `yaml:"type"`
		TokenEnv string `yaml:"token_env"`
		Header   string `yaml:"header"`
	} `yaml:"auth"`
	StripPrefix bool `yaml:"strip_prefix"`
}

type Config struct {
	Listen        string  `yaml:"listen"`
	Routes        []Route `yaml:"routes"`
	DefaultAction string  `yaml:"default_action"`
}

func main() {
	configPath := flag.String("config", "gateway.yaml", "Path to config file")
	flag.Parse()

	config, err := loadConfig(*configPath)
	if err != nil {
		log.Fatalf("Failed to load config: %v", err)
	}

	handler := &Gateway{config: config}

	log.Printf("Auth Gateway starting on %s", config.Listen)
	log.Printf("Loaded %d routes", len(config.Routes))
	for _, r := range config.Routes {
		log.Printf("  %s -> %s", r.Prefix, r.Upstream)
	}

	server := &http.Server{
		Addr:         config.Listen,
		Handler:      handler,
		ReadTimeout:  30 * time.Second,
		WriteTimeout: 30 * time.Second,
	}

	if err := server.ListenAndServe(); err != nil {
		log.Fatalf("Server failed: %v", err)
	}
}

func loadConfig(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	// Expand environment variables in config
	expanded := os.ExpandEnv(string(data))

	var config Config
	if err := yaml.Unmarshal([]byte(expanded), &config); err != nil {
		return nil, err
	}

	if config.Listen == "" {
		config.Listen = ":8080"
	}
	if config.DefaultAction == "" {
		config.DefaultAction = "deny"
	}

	return &config, nil
}

type Gateway struct {
	config *Config
	client *http.Client
}

func (g *Gateway) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	start := time.Now()
	path := r.URL.Path

	// Find matching route
	var route *Route
	for i := range g.config.Routes {
		if strings.HasPrefix(path, g.config.Routes[i].Prefix) {
			route = &g.config.Routes[i]
			break
		}
	}

	if route == nil {
		if g.config.DefaultAction == "deny" {
			log.Printf("DENIED %s %s (no matching route)", r.Method, path)
			http.Error(w, "Route not allowed", http.StatusForbidden)
			return
		}
	}

	// Build upstream URL
	upstreamPath := path
	if route.StripPrefix {
		upstreamPath = strings.TrimPrefix(path, route.Prefix)
		if upstreamPath == "" {
			upstreamPath = "/"
		}
	}

	upstream, err := url.Parse(route.Upstream)
	if err != nil {
		log.Printf("ERROR invalid upstream: %v", err)
		http.Error(w, "Invalid upstream", http.StatusInternalServerError)
		return
	}

	targetURL := upstream.JoinPath(upstreamPath)
	targetURL.RawQuery = r.URL.RawQuery

	// Create upstream request
	upstreamReq, err := http.NewRequest(r.Method, targetURL.String(), r.Body)
	if err != nil {
		log.Printf("ERROR creating request: %v", err)
		http.Error(w, "Failed to create request", http.StatusInternalServerError)
		return
	}

	// Copy headers from original request
	for key, values := range r.Header {
		for _, v := range values {
			upstreamReq.Header.Add(key, v)
		}
	}

	// Inject auth token
	if route.Auth.Type != "" && route.Auth.Type != "none" {
		token := os.Getenv(route.Auth.TokenEnv)
		if token != "" {
			header := route.Auth.Header
			if header == "" {
				header = "Authorization"
			}

			switch route.Auth.Type {
			case "bearer":
				upstreamReq.Header.Set(header, "Bearer "+token)
			case "token":
				upstreamReq.Header.Set(header, token)
			case "basic":
				upstreamReq.SetBasicAuth("", token)
			}
		}
	}

	// Make upstream request
	if g.client == nil {
		g.client = &http.Client{
			Timeout: 60 * time.Second,
		}
	}

	resp, err := g.client.Do(upstreamReq)
	if err != nil {
		log.Printf("ERROR upstream request: %v", err)
		http.Error(w, "Upstream request failed", http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()

	// Copy response headers
	for key, values := range resp.Header {
		for _, v := range values {
			w.Header().Add(key, v)
		}
	}

	// Write status and body
	w.WriteHeader(resp.StatusCode)
	io.Copy(w, resp.Body)

	elapsed := time.Since(start)
	log.Printf("%s %s -> %s [%d] (%v)", r.Method, path, route.Upstream, resp.StatusCode, elapsed)
}

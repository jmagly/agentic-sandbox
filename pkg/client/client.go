package client

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/roctinam/agentic-sandbox/internal/sandbox"
)

// Client is a Go client for the sandbox manager API
type Client struct {
	baseURL    string
	httpClient *http.Client
}

// NewClient creates a new sandbox manager client
func NewClient(baseURL string) *Client {
	return &Client{
		baseURL: baseURL,
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

// Health checks if the server is healthy
func (c *Client) Health(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, "GET", c.baseURL+"/health", nil)
	if err != nil {
		return err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("health check failed with status %d", resp.StatusCode)
	}

	return nil
}

// CreateSandbox creates a new sandbox
func (c *Client) CreateSandbox(ctx context.Context, spec *sandbox.SandboxSpec) (*sandbox.Sandbox, error) {
	body, err := json.Marshal(spec)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequestWithContext(ctx, "POST", c.baseURL+"/api/v1/sandboxes", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusCreated {
		body, _ := io.ReadAll(resp.Body)
		return nil, fmt.Errorf("create failed with status %d: %s", resp.StatusCode, body)
	}

	var sb sandbox.Sandbox
	if err := json.NewDecoder(resp.Body).Decode(&sb); err != nil {
		return nil, err
	}

	return &sb, nil
}

// GetSandbox retrieves a sandbox by ID
func (c *Client) GetSandbox(ctx context.Context, id string) (*sandbox.Sandbox, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", c.baseURL+"/api/v1/sandboxes/"+id, nil)
	if err != nil {
		return nil, err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("get failed with status %d", resp.StatusCode)
	}

	var sb sandbox.Sandbox
	if err := json.NewDecoder(resp.Body).Decode(&sb); err != nil {
		return nil, err
	}

	return &sb, nil
}

// ListSandboxes retrieves all sandboxes
func (c *Client) ListSandboxes(ctx context.Context) ([]*sandbox.Sandbox, error) {
	req, err := http.NewRequestWithContext(ctx, "GET", c.baseURL+"/api/v1/sandboxes", nil)
	if err != nil {
		return nil, err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("list failed with status %d", resp.StatusCode)
	}

	var sandboxes []*sandbox.Sandbox
	if err := json.NewDecoder(resp.Body).Decode(&sandboxes); err != nil {
		return nil, err
	}

	return sandboxes, nil
}

// StartSandbox starts a sandbox
func (c *Client) StartSandbox(ctx context.Context, id string) error {
	req, err := http.NewRequestWithContext(ctx, "POST", c.baseURL+"/api/v1/sandboxes/"+id+"/start", nil)
	if err != nil {
		return err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusNoContent {
		return fmt.Errorf("start failed with status %d", resp.StatusCode)
	}

	return nil
}

// StopSandbox stops a sandbox
func (c *Client) StopSandbox(ctx context.Context, id string) error {
	req, err := http.NewRequestWithContext(ctx, "POST", c.baseURL+"/api/v1/sandboxes/"+id+"/stop", nil)
	if err != nil {
		return err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusNoContent {
		return fmt.Errorf("stop failed with status %d", resp.StatusCode)
	}

	return nil
}

// DeleteSandbox deletes a sandbox
func (c *Client) DeleteSandbox(ctx context.Context, id string) error {
	req, err := http.NewRequestWithContext(ctx, "DELETE", c.baseURL+"/api/v1/sandboxes/"+id, nil)
	if err != nil {
		return err
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusNoContent {
		return fmt.Errorf("delete failed with status %d", resp.StatusCode)
	}

	return nil
}

// TODO: Add methods for:
// - Exec(ctx, id, cmd) - Execute command in sandbox
// - Logs(ctx, id) - Get sandbox logs
// - Stats(ctx, id) - Get resource usage stats

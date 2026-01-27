package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"text/tabwriter"
	"time"

	"github.com/roctinam/agentic-sandbox/internal/sandbox"
	"github.com/roctinam/agentic-sandbox/pkg/client"
)

const (
	defaultServerURL = "http://localhost:8080"
)

func main() {
	if len(os.Args) < 2 {
		printUsage()
		os.Exit(1)
	}

	command := os.Args[1]

	switch command {
	case "create":
		handleCreate(os.Args[2:])
	case "start":
		handleStart(os.Args[2:])
	case "stop":
		handleStop(os.Args[2:])
	case "delete":
		handleDelete(os.Args[2:])
	case "get":
		handleGet(os.Args[2:])
	case "list":
		handleList(os.Args[2:])
	case "exec":
		handleExec(os.Args[2:])
	case "health":
		handleHealth(os.Args[2:])
	default:
		fmt.Fprintf(os.Stderr, "Unknown command: %s\n", command)
		printUsage()
		os.Exit(1)
	}
}

func printUsage() {
	fmt.Println("Usage: sandbox-cli [command] [options]")
	fmt.Println()
	fmt.Println("Commands:")
	fmt.Println("  create    Create a new sandbox")
	fmt.Println("  start     Start a sandbox")
	fmt.Println("  stop      Stop a sandbox")
	fmt.Println("  delete    Delete a sandbox")
	fmt.Println("  get       Get sandbox details")
	fmt.Println("  list      List all sandboxes")
	fmt.Println("  exec      Execute command in sandbox (TODO)")
	fmt.Println("  health    Check server health")
	fmt.Println()
	fmt.Println("Environment variables:")
	fmt.Println("  SANDBOX_SERVER_URL  Server URL (default: http://localhost:8080)")
}

func getClient() *client.Client {
	serverURL := os.Getenv("SANDBOX_SERVER_URL")
	if serverURL == "" {
		serverURL = defaultServerURL
	}
	return client.NewClient(serverURL)
}

func handleHealth(args []string) {
	c := getClient()
	ctx := context.Background()

	if err := c.Health(ctx); err != nil {
		fmt.Fprintf(os.Stderr, "Health check failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("Server is healthy")
}

func handleCreate(args []string) {
	fs := flag.NewFlagSet("create", flag.ExitOnError)
	name := fs.String("name", "", "Sandbox name (required)")
	runtime := fs.String("runtime", "docker", "Runtime type (docker/qemu)")
	image := fs.String("image", "agent-claude", "Image name")
	cpu := fs.String("cpu", "4", "CPU count")
	memory := fs.String("memory", "8G", "Memory limit")
	pidsLimit := fs.Int("pids-limit", 1024, "PID limit")
	network := fs.String("network", "isolated", "Network mode (isolated/gateway/host)")
	gatewayURL := fs.String("gateway", "", "Gateway URL")
	autoStart := fs.Bool("auto-start", false, "Auto-start after creation")

	fs.Parse(args)

	if *name == "" {
		fmt.Fprintln(os.Stderr, "Error: --name is required")
		fs.PrintDefaults()
		os.Exit(1)
	}

	spec := &sandbox.SandboxSpec{
		Name:    *name,
		Runtime: *runtime,
		Image:   *image,
		Resources: sandbox.Resources{
			CPU:       *cpu,
			Memory:    *memory,
			PidsLimit: *pidsLimit,
		},
		Network:    sandbox.NetworkMode(*network),
		GatewayURL: *gatewayURL,
		AutoStart:  *autoStart,
	}

	c := getClient()
	ctx := context.Background()

	sb, err := c.CreateSandbox(ctx, spec)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to create sandbox: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Sandbox created: %s\n", sb.ID)
	printSandbox(sb)
}

func handleStart(args []string) {
	if len(args) < 1 {
		fmt.Fprintln(os.Stderr, "Error: sandbox ID is required")
		os.Exit(1)
	}

	id := args[0]
	c := getClient()
	ctx := context.Background()

	if err := c.StartSandbox(ctx, id); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to start sandbox: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Sandbox %s started\n", id)
}

func handleStop(args []string) {
	if len(args) < 1 {
		fmt.Fprintln(os.Stderr, "Error: sandbox ID is required")
		os.Exit(1)
	}

	id := args[0]
	c := getClient()
	ctx := context.Background()

	if err := c.StopSandbox(ctx, id); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to stop sandbox: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Sandbox %s stopped\n", id)
}

func handleDelete(args []string) {
	if len(args) < 1 {
		fmt.Fprintln(os.Stderr, "Error: sandbox ID is required")
		os.Exit(1)
	}

	id := args[0]
	c := getClient()
	ctx := context.Background()

	if err := c.DeleteSandbox(ctx, id); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to delete sandbox: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Sandbox %s deleted\n", id)
}

func handleGet(args []string) {
	if len(args) < 1 {
		fmt.Fprintln(os.Stderr, "Error: sandbox ID is required")
		os.Exit(1)
	}

	id := args[0]
	c := getClient()
	ctx := context.Background()

	sb, err := c.GetSandbox(ctx, id)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to get sandbox: %v\n", err)
		os.Exit(1)
	}

	printSandbox(sb)
}

func handleList(args []string) {
	fs := flag.NewFlagSet("list", flag.ExitOnError)
	jsonOutput := fs.Bool("json", false, "Output as JSON")
	fs.Parse(args)

	c := getClient()
	ctx := context.Background()

	sandboxes, err := c.ListSandboxes(ctx)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Failed to list sandboxes: %v\n", err)
		os.Exit(1)
	}

	if *jsonOutput {
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		enc.Encode(sandboxes)
		return
	}

	w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
	fmt.Fprintln(w, "ID\tNAME\tRUNTIME\tIMAGE\tSTATE\tCREATED")
	for _, sb := range sandboxes {
		created := sb.CreatedAt.Format(time.RFC3339)
		fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%s\t%s\n",
			sb.ID, sb.Name, sb.Runtime, sb.Image, sb.State, created)
	}
	w.Flush()
}

func handleExec(args []string) {
	fmt.Fprintln(os.Stderr, "TODO: exec command not yet implemented")
	os.Exit(1)
}

func printSandbox(sb *sandbox.Sandbox) {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	enc.Encode(sb)
}

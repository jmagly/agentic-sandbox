# Multi-Agent Orchestration Guide

Quick reference for using multi-agent orchestration features in the agentic-sandbox management server.

## Table of Contents

1. [Overview](#overview)
2. [Task Manifest Configuration](#task-manifest-configuration)
3. [API Reference](#api-reference)
4. [Common Patterns](#common-patterns)
5. [Error Handling](#error-handling)

## Overview

The multi-agent orchestration system enables parent-child task delegation, allowing complex workflows to be broken down into coordinated subtasks.

**Key Features:**
- Parent-child relationship tracking
- Automatic status aggregation
- Artifact collection from children
- Wait-for-children blocking
- Concurrent execution limits

## Task Manifest Configuration

### Parent Task

A parent task can spawn and coordinate multiple child tasks:

```yaml
version: "1"
kind: Task
metadata:
  id: "parent-task-001"
  name: "Multi-Step Analysis"
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Coordinate the multi-step analysis workflow"
children:
  max_concurrent: 3              # Run max 3 children at once (optional)
  wait_for_children: true        # Block parent completion until all children done
  aggregate_artifacts: true      # Collect all child artifacts into parent's outbox
lifecycle:
  timeout: "2h"
  artifact_patterns:
    - "summary-*.md"             # Parent's own artifacts
```

### Child Task

A child task references its parent:

```yaml
version: "1"
kind: Task
metadata:
  id: "child-task-001"
  name: "Step 1: Data Collection"
parent_id: "parent-task-001"     # Links to parent
repository:
  url: "https://github.com/example/repo.git"
  branch: "main"
claude:
  prompt: "Collect and prepare data for analysis"
lifecycle:
  artifact_patterns:
    - "data-*.json"              # Child's artifacts
```

## API Reference

### ParentChildTracker

Manages parent-child relationships.

```rust
use agentic_management::orchestrator::ParentChildTracker;

// Create tracker
let tracker = ParentChildTracker::new(checkpoint_store);

// Register a child
tracker.register_child("parent-id", "child-id").await?;

// Get all children of a parent
let children = tracker.get_children("parent-id").await;

// Get parent of a child
let parent = tracker.get_parent("child-id").await;

// Get aggregated status
let status = tracker.get_children_status("parent-id").await?;
println!("Progress: {}%", status.completion_percentage());

// Wait for all children to complete
let completed = tracker.wait_for_children("parent-id").await?;

// Cleanup relationship
tracker.unregister_child("child-id").await?;
```

### ChildrenStatus

Aggregated status information.

```rust
use agentic_management::orchestrator::ChildrenStatus;

let status = tracker.get_children_status("parent-id").await?;

// Check counts
println!("Total: {}", status.total);
println!("Running: {}", status.running);
println!("Completed: {}", status.completed);
println!("Failed: {}", status.failed);

// Check conditions
if status.all_terminal() {
    println!("All children are done");
}

if status.has_failures() {
    println!("Some children failed");
}

// Get completion percentage
let progress = status.completion_percentage(); // 0.0 to 100.0
```

### ArtifactCollector

Collects and aggregates artifacts.

```rust
use agentic_management::orchestrator::ArtifactCollector;

let collector = ArtifactCollector::new();

// Aggregate child artifacts into parent's outbox
let result = collector.aggregate_child_artifacts(
    "parent-id",
    &["child-1".to_string(), "child-2".to_string()],
).await?;

println!("Artifacts collected: {}", result.artifacts_collected);
println!("Total bytes: {}", result.bytes_collected);
println!("Errors: {:?}", result.errors);
```

Artifacts are organized by child:

```
/tasks/parent-id/outbox/child-artifacts/
  ├── child-1/
  │   ├── data.json
  │   └── report.txt
  └── child-2/
      └── analysis.csv
```

## Common Patterns

### Pattern 1: Fan-Out Processing

Parent spawns multiple children to process work in parallel.

**Use case:** Process multiple files, analyze different datasets, run parallel experiments.

```yaml
# parent-manifest.yaml
children:
  max_concurrent: 5
  wait_for_children: true
  aggregate_artifacts: true
```

**Workflow:**
1. Parent task starts
2. Parent spawns N child tasks
3. Children run in parallel (max 5 at once)
4. Parent waits for all children
5. Parent aggregates all artifacts
6. Parent completes

### Pattern 2: Sequential Pipeline

Parent spawns children in sequence, each depending on the previous.

**Use case:** ETL pipelines, multi-stage builds, sequential analysis.

```yaml
# parent-manifest.yaml
children:
  max_concurrent: 1              # One at a time
  wait_for_children: true
  aggregate_artifacts: true
```

**Workflow:**
1. Parent spawns child-1
2. Wait for child-1 to complete
3. Parent spawns child-2
4. Wait for child-2 to complete
5. Continue...
6. Parent aggregates results

### Pattern 3: Fire-and-Forget

Parent spawns children but doesn't wait for completion.

**Use case:** Async notifications, background tasks, monitoring.

```yaml
# parent-manifest.yaml
children:
  wait_for_children: false       # Don't wait
  aggregate_artifacts: false     # Don't aggregate
```

**Workflow:**
1. Parent spawns children
2. Parent continues immediately
3. Parent completes independently
4. Children complete on their own

### Pattern 4: Best-Effort Collection

Parent waits for children and tries to collect artifacts, but tolerates failures.

**Use case:** Research tasks, exploratory analysis, optional steps.

```yaml
# parent-manifest.yaml
children:
  wait_for_children: true
  aggregate_artifacts: true
lifecycle:
  failure_action: "preserve"     # Preserve VM on failure for debugging
```

**Workflow:**
1. Parent spawns children
2. Some children may fail
3. Parent waits for all to complete
4. Parent aggregates available artifacts
5. Parent completes with partial results

## Error Handling

### MultiAgentError Types

```rust
use agentic_management::orchestrator::MultiAgentError;

match result {
    Err(MultiAgentError::TaskNotFound(id)) => {
        println!("Task {} not found", id);
    }
    Err(MultiAgentError::ParentNotFound(id)) => {
        println!("Parent task {} not found", id);
    }
    Err(MultiAgentError::ArtifactError(msg)) => {
        println!("Artifact aggregation failed: {}", msg);
    }
    Err(MultiAgentError::Checkpoint(err)) => {
        println!("Checkpoint error: {}", err);
    }
    Ok(result) => {
        println!("Success: {:?}", result);
    }
}
```

### Handling Partial Failures

When aggregating artifacts, check the errors field:

```rust
let result = collector.aggregate_child_artifacts(parent_id, child_ids).await?;

if !result.errors.is_empty() {
    warn!("Aggregation had {} errors:", result.errors.len());
    for error in &result.errors {
        warn!("  - {}", error);
    }
}

println!(
    "Successfully collected {} artifacts from {} children",
    result.artifacts_collected,
    result.children_processed
);
```

### Child Failure Handling

Check children status to handle failures:

```rust
let status = tracker.get_children_status(parent_id).await?;

if status.has_failures() {
    error!(
        "{} children failed out of {}",
        status.failed,
        status.total
    );

    // Get all children to see which ones failed
    let children = tracker.wait_for_children(parent_id).await?;
    for child in children {
        if matches!(child.state, TaskState::Failed) {
            error!("Child {} failed: {:?}", child.id, child.error);
        }
    }
}
```

## Advanced Usage

### Custom Concurrency Control

```rust
// Track running children manually
let mut running = 0;
let max_concurrent = 3;

for child_manifest in child_manifests {
    // Wait if at capacity
    while running >= max_concurrent {
        tokio::time::sleep(Duration::from_secs(5)).await;

        let status = tracker.get_children_status(parent_id).await?;
        running = status.running;
    }

    // Spawn child
    let child_id = orchestrator.submit_task(child_manifest).await?;
    tracker.register_child(parent_id, &child_id).await?;
    running += 1;
}
```

### Progress Monitoring

```rust
// Poll children status periodically
let interval = Duration::from_secs(10);
let mut ticker = tokio::time::interval(interval);

loop {
    ticker.tick().await;

    let status = tracker.get_children_status(parent_id).await?;

    info!(
        "Progress: {:.1}% ({}/{} complete, {} failed)",
        status.completion_percentage(),
        status.completed,
        status.total,
        status.failed
    );

    if status.all_terminal() {
        break;
    }
}
```

### Selective Aggregation

```rust
// Only aggregate from successful children
let children = tracker.wait_for_children(parent_id).await?;

let successful_children: Vec<String> = children
    .iter()
    .filter(|c| c.state == TaskState::Completed)
    .map(|c| c.id.clone())
    .collect();

let result = collector.aggregate_child_artifacts(
    parent_id,
    &successful_children,
).await?;
```

## Best Practices

1. **Always set timeouts**: Parent tasks with `wait_for_children: true` should have generous timeouts

2. **Limit concurrency**: Use `max_concurrent` to avoid resource exhaustion

3. **Handle failures gracefully**: Check `has_failures()` and decide how to proceed

4. **Monitor progress**: Poll `get_children_status()` for long-running workflows

5. **Clean up relationships**: Call `unregister_child()` for cancelled tasks

6. **Use artifact patterns**: Configure specific patterns to avoid collecting too much

7. **Test failure scenarios**: Test what happens when children fail

8. **Consider VM resources**: Each child gets its own VM - plan capacity accordingly

## Troubleshooting

### Parent stuck in "Running" state

**Cause:** `wait_for_children: true` but children aren't completing

**Solution:**
```rust
// Check children status
let status = tracker.get_children_status(parent_id).await?;
println!("Status: {:?}", status);

// List all children
let children = tracker.get_children(parent_id).await;
for child_id in children {
    let task = orchestrator.get_task(&child_id).await;
    println!("Child {}: {:?}", child_id, task.map(|t| t.state));
}
```

### Artifacts not aggregating

**Cause:** Children didn't create artifacts or aggregation failed

**Solution:**
```rust
// Check aggregation result
let result = collector.aggregate_child_artifacts(parent_id, child_ids).await?;
println!("Collected: {}", result.artifacts_collected);
println!("Errors: {:?}", result.errors);

// Verify children created artifacts
for child_id in child_ids {
    let artifacts = collector.list_artifacts(&child_id).await?;
    println!("Child {}: {} artifacts", child_id, artifacts.len());
}
```

### Too many concurrent VMs

**Cause:** `max_concurrent` not set or too high

**Solution:**
```yaml
children:
  max_concurrent: 3  # Lower limit
```

## Examples

See `MULTI_AGENT_IMPLEMENTATION.md` for detailed examples and the test suite in `src/orchestrator/multi_agent.rs` for usage patterns.

## Support

For issues or questions:
- File an issue: https://git.integrolabs.net/roctinam/agentic-sandbox/issues
- Check tests: `src/orchestrator/multi_agent.rs`
- Review implementation: `MULTI_AGENT_IMPLEMENTATION.md`

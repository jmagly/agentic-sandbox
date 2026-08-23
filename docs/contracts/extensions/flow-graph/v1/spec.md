# AIWG Flow graph node extension v1

URI: `https://aiwg.io/extensions/flow-graph/v1`

This optional A2A extension lets an AIWG Flow graph dispatch one Sandbox task
as a graph node. Sandbox remains a node executor: it does not evaluate routes,
guards, reducers, graph budgets, or successor selection.

## Activation and request

The client includes the URI in `A2A-Extensions` and supplies the same URI as a
key in `message.metadata`. The value contains the string fields `graph_id`,
`graph_version`, `run_id`, `node_id`, and `node_run_id`; `edge_id` is optional.
Unknown fields, control characters, empty identifiers, and values longer than
256 bytes are rejected with `flow_graph.invalid_metadata`. Ordinary A2A calls
that do not activate the extension are unchanged.

The namespace is identity-only. It must not carry prompts, task content,
private reasoning, credentials, authorization material, or raw tool output.

## Task projection

The Task response preserves the identity object at
`Task.metadata[https://aiwg.io/extensions/flow-graph/v1]`. The latest
schema-shaped `GraphSandboxNodeEvent` is at `Task.metadata[flow_graph.event]`.
It binds graph identity to Sandbox task/session identity, the stable A2A
message idempotency key, runtime binding `a2a-sandbox`, and one lifecycle,
terminal, or checkpoint event.

Lifecycle mapping is `submitted -> queued` and `working -> running`. Terminal
records distinguish `succeeded`, `failed`, `canceled`, `timed_out`, and
`unknown`. A worker/output disconnect without a durable terminal frame is
`unknown`; it is never inferred as success and never silently retried.

Available output evidence carries a `sandbox://` reference, SHA-256 digest,
and explicit redaction status. Raw output remains in the separately authorized
artifact API and is not copied into graph metadata. Missing evidence cannot
prove success.

## Checkpoint events

After the runtime performs a checkpoint or restore, an authorized bridge posts
the result to:

`POST /agents/{instance_id}/tasks/{task_id}/graph-checkpoints`

The extension must be activated. `created` and `restored` require
`checkpoint_id`, lowercase `sha256:<64 hex>` `checkpoint_digest`, and
`resumability`. `restore_failed` requires `reason` and `resumability`. This
endpoint records an observed runtime result; it does not itself initiate a VM
checkpoint.

## Idempotency and replay

Activate the existing `idempotency/v1` extension and use a stable A2A
`messageId` for each node run. Identical replay returns the prior Task; reuse
with different canonical request bytes is rejected. A resume operation must
preserve graph/run/node identity and explicitly link the prior task and
checkpoint in `message.metadata[flow_graph.resume]` using the exact fields
`replay_of_task_id` and `checkpoint_id`; these are copied into the durable task
identity record. Non-resumable or unknown outcomes need
operator-visible reconciliation before any new side-effecting dispatch.

The canonical cross-repository event schema is maintained by AIWG at
`schemas/flow/graph-sandbox-node-event.v1.schema.json`.

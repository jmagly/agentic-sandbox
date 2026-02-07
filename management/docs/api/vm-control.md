# VM Control API Design

Version: 1.0.0-draft
Status: Proposed
Last Updated: 2026-02-01

## Overview

This document specifies the VM lifecycle control API for the Agentic Sandbox management server. The API provides programmatic control over QEMU/KVM virtual machines via libvirt, enabling UI dashboards and automation systems to provision, start, stop, restart, and destroy agent VMs.

## Design Principles

1. **Resource-Oriented Design** - VMs are treated as resources following REST conventions
2. **Custom Methods for Actions** - State-changing operations use `POST /resource/{id}:action` pattern (GCP-style)
3. **Idempotency** - All mutating operations support idempotency keys to prevent duplicate actions
4. **Async Operations** - Long-running operations return immediately with operation tracking
5. **State Machine Enforcement** - Invalid state transitions are rejected with clear error messages
6. **Unified Event Stream** - All VM operations emit events to the existing event system

## VM State Machine

```
                          ┌─────────────────────────────────────────┐
                          │                                         │
                          ▼                                         │
  ┌───────────┐    ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
  │ undefined │───▶│ provisioning│───▶│   stopped   │◀──▶│   running   │
  └───────────┘    └─────────────┘    └─────────────┘    └─────────────┘
        ▲                │                   │                   │
        │                │                   │                   │
        │                ▼                   ▼                   ▼
        │         ┌─────────────┐    ┌─────────────┐    ┌─────────────┐
        └─────────│   failed    │    │  destroyed  │    │   crashed   │
                  └─────────────┘    └─────────────┘    └─────────────┘
```

### States

| State | Description |
|-------|-------------|
| `undefined` | VM does not exist in libvirt |
| `provisioning` | VM is being created (disk, cloud-init, define) |
| `stopped` | VM is defined but not running |
| `running` | VM is actively executing |
| `crashed` | VM terminated unexpectedly |
| `destroyed` | VM was force-stopped (transitional to stopped) |
| `failed` | Provisioning or operation failed |

### Valid Transitions

| From | To | Trigger |
|------|-----|---------|
| undefined | provisioning | `POST /vms` (create) |
| provisioning | stopped | Provisioning completes |
| provisioning | failed | Provisioning error |
| stopped | running | `POST /vms/{name}:start` |
| running | stopped | `POST /vms/{name}:stop` (graceful) |
| running | destroyed | `POST /vms/{name}:destroy` (force) |
| running | crashed | Unexpected termination |
| running | running | `POST /vms/{name}:restart` |
| crashed | running | `POST /vms/{name}:start` |
| stopped | undefined | `DELETE /vms/{name}` |

## API Endpoints

### Base Path

All VM control endpoints are under `/api/v1/vms`.

### List VMs

```
GET /api/v1/vms
```

Returns all VMs managed by libvirt with optional filtering.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `state` | string | Filter by state: `running`, `stopped`, `all` |
| `prefix` | string | Filter by name prefix (default: `agent-`) |
| `page` | integer | Page number (1-indexed) |
| `per_page` | integer | Items per page (default: 50, max: 100) |

**Response:**

```json
{
  "vms": [
    {
      "name": "agent-01",
      "state": "running",
      "uuid": "a1b2c3d4-...",
      "vcpus": 4,
      "memory_mb": 8192,
      "disk_gb": 50,
      "ip_address": "192.168.122.201",
      "uptime_seconds": 3600,
      "created_at": "2026-02-01T10:00:00Z",
      "agent_connected": true
    }
  ],
  "total": 1,
  "page": 1,
  "per_page": 50
}
```

### Get VM Details

```
GET /api/v1/vms/{name}
```

Returns detailed information about a specific VM.

**Response:**

```json
{
  "name": "agent-01",
  "state": "running",
  "uuid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "vcpus": 4,
  "memory_mb": 8192,
  "disk_path": "/var/lib/libvirt/images/agent-01.qcow2",
  "disk_gb": 50,
  "ip_address": "192.168.122.201",
  "mac_address": "52:54:00:xx:xx:xx",
  "network": "default",
  "uptime_seconds": 3600,
  "created_at": "2026-02-01T10:00:00Z",
  "profile": "agentic-dev",
  "agentshare_enabled": true,
  "agent": {
    "connected": true,
    "connected_at": "2026-02-01T10:01:30Z",
    "hostname": "agent-01",
    "version": "0.1.0"
  }
}
```

### Create VM

```
POST /api/v1/vms
```

Provisions a new VM using the provisioning system.

**Request Headers:**

| Header | Description |
|--------|-------------|
| `Idempotency-Key` | Unique key to prevent duplicate provisioning |

**Request Body:**

```json
{
  "name": "agent-02",
  "profile": "agentic-dev",
  "vcpus": 4,
  "memory_mb": 8192,
  "disk_gb": 50,
  "agentshare": true,
  "start": true
}
```

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | - | VM name (must match `^agent-[a-z0-9-]+$`) |
| `profile` | string | no | `agentic-dev` | Provisioning profile |
| `vcpus` | integer | no | 4 | Number of virtual CPUs |
| `memory_mb` | integer | no | 8192 | Memory in megabytes |
| `disk_gb` | integer | no | 50 | Disk size in gigabytes |
| `agentshare` | boolean | no | true | Enable agentshare mounts |
| `start` | boolean | no | true | Start VM after provisioning |

**Response (202 Accepted):**

```json
{
  "operation": {
    "id": "op-abc123",
    "type": "vm.create",
    "state": "running",
    "target": "agent-02",
    "created_at": "2026-02-01T10:00:00Z",
    "progress_percent": 0
  },
  "vm": {
    "name": "agent-02",
    "state": "provisioning"
  }
}
```

### Start VM

```
POST /api/v1/vms/{name}:start
```

Starts a stopped VM.

**Request Headers:**

| Header | Description |
|--------|-------------|
| `Idempotency-Key` | Unique key to prevent duplicate starts |

**Request Body (optional):**

```json
{
  "wait": true,
  "timeout_seconds": 120
}
```

**Response (200 OK - if already running):**

```json
{
  "vm": {
    "name": "agent-01",
    "state": "running"
  },
  "message": "VM is already running"
}
```

**Response (202 Accepted - if starting):**

```json
{
  "operation": {
    "id": "op-def456",
    "type": "vm.start",
    "state": "running",
    "target": "agent-01",
    "created_at": "2026-02-01T10:05:00Z"
  },
  "vm": {
    "name": "agent-01",
    "state": "starting"
  }
}
```

### Stop VM

```
POST /api/v1/vms/{name}:stop
```

Initiates graceful shutdown via ACPI.

**Request Headers:**

| Header | Description |
|--------|-------------|
| `Idempotency-Key` | Unique key to prevent duplicate stops |

**Request Body (optional):**

```json
{
  "timeout_seconds": 60,
  "force_after_timeout": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `timeout_seconds` | integer | 60 | Seconds to wait for graceful shutdown |
| `force_after_timeout` | boolean | false | Force destroy if timeout exceeded |

**Response (202 Accepted):**

```json
{
  "operation": {
    "id": "op-ghi789",
    "type": "vm.stop",
    "state": "running",
    "target": "agent-01",
    "created_at": "2026-02-01T10:10:00Z"
  },
  "vm": {
    "name": "agent-01",
    "state": "stopping"
  }
}
```

### Restart VM

```
POST /api/v1/vms/{name}:restart
```

Restarts the VM (stop + start cycle).

**Request Body (optional):**

```json
{
  "mode": "graceful",
  "timeout_seconds": 60
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `graceful` | `graceful` (ACPI shutdown) or `hard` (destroy+start) |
| `timeout_seconds` | integer | 60 | Timeout for graceful shutdown phase |

**Response (202 Accepted):**

```json
{
  "operation": {
    "id": "op-jkl012",
    "type": "vm.restart",
    "state": "running",
    "target": "agent-01",
    "created_at": "2026-02-01T10:15:00Z"
  }
}
```

### Destroy VM (Force Stop)

```
POST /api/v1/vms/{name}:destroy
```

Immediately terminates the VM (equivalent to pulling the power plug).

**Response (200 OK):**

```json
{
  "vm": {
    "name": "agent-01",
    "state": "stopped"
  },
  "message": "VM destroyed"
}
```

### Delete VM

```
DELETE /api/v1/vms/{name}
```

Undefines and removes the VM from libvirt. The VM must be stopped.

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `delete_disk` | boolean | false | Also delete the disk image |
| `force` | boolean | false | Force destroy if running, then delete |

**Response (200 OK):**

```json
{
  "deleted": true,
  "name": "agent-01",
  "disk_deleted": false
}
```

**Response (409 Conflict - if running):**

```json
{
  "error": {
    "code": "VM_RUNNING",
    "message": "Cannot delete running VM. Stop it first or use force=true",
    "vm_state": "running"
  }
}
```

### Get Operation Status

```
GET /api/v1/operations/{id}
```

Returns the status of a long-running operation.

**Response:**

```json
{
  "id": "op-abc123",
  "type": "vm.create",
  "state": "completed",
  "target": "agent-02",
  "created_at": "2026-02-01T10:00:00Z",
  "completed_at": "2026-02-01T10:02:30Z",
  "progress_percent": 100,
  "result": {
    "vm": {
      "name": "agent-02",
      "state": "running",
      "ip_address": "192.168.122.202"
    }
  }
}
```

**Operation States:**

| State | Description |
|-------|-------------|
| `pending` | Operation queued |
| `running` | Operation in progress |
| `completed` | Operation succeeded |
| `failed` | Operation failed |
| `cancelled` | Operation was cancelled |

## Error Handling

All errors follow a consistent format:

```json
{
  "error": {
    "code": "ERROR_CODE",
    "message": "Human-readable message",
    "details": { ... }
  }
}
```

### Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `VM_NOT_FOUND` | 404 | VM does not exist |
| `VM_RUNNING` | 409 | VM is running (for operations requiring stopped state) |
| `VM_STOPPED` | 409 | VM is already stopped (for stop operation) |
| `VM_ALREADY_EXISTS` | 409 | VM with name already exists |
| `INVALID_STATE_TRANSITION` | 409 | Requested operation invalid for current state |
| `OPERATION_IN_PROGRESS` | 409 | Another operation is in progress for this VM |
| `PROVISIONING_FAILED` | 500 | VM provisioning failed |
| `LIBVIRT_ERROR` | 500 | Underlying libvirt error |
| `INVALID_VM_NAME` | 400 | VM name doesn't match required pattern |
| `RESOURCE_LIMIT_EXCEEDED` | 400 | Requested resources exceed limits |

## Idempotency

All mutating operations (`POST`, `DELETE`) support idempotency via the `Idempotency-Key` header.

**Behavior:**
- First request with key: Execute operation, cache result
- Subsequent requests with same key within 24 hours: Return cached result
- Key format: Any string up to 255 characters (recommend UUID)

**Example:**

```bash
# First request - starts the VM
curl -X POST http://localhost:8122/api/v1/vms/agent-01:start \
  -H "Idempotency-Key: start-agent01-$(date +%Y%m%d)"

# Duplicate request - returns same result, no double-start
curl -X POST http://localhost:8122/api/v1/vms/agent-01:start \
  -H "Idempotency-Key: start-agent01-$(date +%Y%m%d)"
```

## Events

All VM operations emit events to the existing event system, visible in the dashboard sidebar and via `/api/v1/events`.

| Operation | Event Type |
|-----------|------------|
| Create | `vm.provisioning`, `vm.defined`, `vm.started` |
| Start | `vm.started` |
| Stop | `vm.stopped` |
| Restart | `vm.stopped`, `vm.started` |
| Destroy | `vm.stopped` (with reason=destroyed) |
| Delete | `vm.undefined` |

## Rate Limits

| Endpoint | Rate Limit |
|----------|------------|
| `GET /vms` | 60/minute |
| `GET /vms/{name}` | 120/minute |
| `POST /vms` | 10/minute |
| `POST /vms/{name}:*` | 30/minute per VM |
| `DELETE /vms/{name}` | 10/minute |

## Implementation Phases

### Phase 1: Core Operations (MVP)
- [ ] List VMs with libvirt integration
- [ ] Get VM details
- [ ] Start VM (virsh start)
- [ ] Stop VM (virsh shutdown)
- [ ] Destroy VM (virsh destroy)
- [ ] Basic error handling

### Phase 2: Full CRUD
- [ ] Create VM (provision-vm.sh integration)
- [ ] Delete VM with disk cleanup option
- [ ] Restart operation (compound stop+start)
- [ ] Operation status tracking

### Phase 3: Advanced Features
- [ ] Idempotency key support
- [ ] Rate limiting
- [ ] Async operation with progress
- [ ] VM resource modification

## Security Considerations

1. **Authentication** - All endpoints require valid session/token
2. **Authorization** - VM operations may be scoped to user/project
3. **Audit Logging** - All operations logged with actor, timestamp, result
4. **Input Validation** - VM names validated against strict pattern
5. **Resource Limits** - Prevent resource exhaustion via quotas

## References

- [Google Cloud Compute API](https://cloud.google.com/compute/docs/reference/rest/v1/instances)
- [libvirt Domain Lifecycle](https://libvirt.org/html/libvirt-libvirt-domain.html)
- [Stripe Idempotency](https://stripe.com/docs/api/idempotent_requests)

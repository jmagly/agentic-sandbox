# VM Control API Phase 2 - Quick Reference

## Create VM

```bash
curl -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{
    "name": "agent-02",
    "profile": "agentic-dev",
    "vcpus": 4,
    "memory_mb": 8192,
    "disk_gb": 50,
    "agentshare": true,
    "start": true
  }'
```

Response (202 Accepted):
```json
{
  "operation": {
    "id": "op-abc123",
    "type": "vm_create",
    "status": "pending",
    "target": "agent-02",
    "created_at": "2026-02-01T10:00:00Z",
    "progress_percent": 0
  }
}
```

## Delete VM

```bash
# Delete without disk
curl -X DELETE http://localhost:8122/api/v1/vms/agent-02

# Delete with disk cleanup
curl -X DELETE 'http://localhost:8122/api/v1/vms/agent-02?delete_disk=true'

# Force delete running VM
curl -X DELETE 'http://localhost:8122/api/v1/vms/agent-02?force=true&delete_disk=true'
```

Response (200 OK):
```json
{
  "deleted": true,
  "name": "agent-02",
  "disk_deleted": true
}
```

## Restart VM

```bash
# Graceful restart
curl -X POST http://localhost:8122/api/v1/vms/agent-01:restart \
  -H "Content-Type: application/json" \
  -d '{
    "mode": "graceful",
    "timeout_seconds": 60
  }'

# Hard restart (force)
curl -X POST http://localhost:8122/api/v1/vms/agent-01:restart \
  -H "Content-Type: application/json" \
  -d '{
    "mode": "hard",
    "timeout_seconds": 30
  }'
```

Response (202 Accepted):
```json
{
  "operation": {
    "id": "op-def456",
    "type": "vm_restart",
    "status": "pending",
    "target": "agent-01",
    "created_at": "2026-02-01T10:05:00Z",
    "progress_percent": 0
  }
}
```

## Check Operation Status

```bash
curl http://localhost:8122/api/v1/operations/op-abc123
```

Response (200 OK):
```json
{
  "id": "op-abc123",
  "type": "vm_create",
  "status": "completed",
  "target": "agent-02",
  "created_at": "2026-02-01T10:00:00Z",
  "completed_at": "2026-02-01T10:02:30Z",
  "progress_percent": 100,
  "result": {
    "vm": {
      "name": "agent-02",
      "state": "running"
    }
  }
}
```

## Error Responses

```bash
# VM already exists
curl -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{"name": "agent-01"}'
```

Response (409 Conflict):
```json
{
  "error": {
    "code": "VM_ALREADY_EXISTS",
    "message": "VM already exists: agent-01"
  }
}
```

```bash
# Invalid VM name
curl -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{"name": "my-vm"}'
```

Response (400 Bad Request):
```json
{
  "error": {
    "code": "INVALID_VM_NAME",
    "message": "VM name 'my-vm' must match pattern '^agent-[a-z0-9-]+$'"
  }
}
```

```bash
# Delete running VM without force
curl -X DELETE http://localhost:8122/api/v1/vms/agent-01
```

Response (409 Conflict):
```json
{
  "error": {
    "code": "VM_RUNNING",
    "message": "Cannot delete running VM: agent-01"
  }
}
```

## Complete Workflow Example

```bash
# 1. Create a new VM
RESPONSE=$(curl -s -X POST http://localhost:8122/api/v1/vms \
  -H "Content-Type: application/json" \
  -d '{
    "name": "agent-test",
    "profile": "agentic-dev",
    "start": true
  }')

# Extract operation ID
OPERATION_ID=$(echo $RESPONSE | jq -r '.operation.id')
echo "Operation ID: $OPERATION_ID"

# 2. Poll operation status
while true; do
  STATUS=$(curl -s http://localhost:8122/api/v1/operations/$OPERATION_ID)
  STATE=$(echo $STATUS | jq -r '.status')
  PROGRESS=$(echo $STATUS | jq -r '.progress_percent')
  
  echo "State: $STATE, Progress: $PROGRESS%"
  
  if [ "$STATE" = "completed" ] || [ "$STATE" = "failed" ]; then
    break
  fi
  
  sleep 5
done

# 3. Check VM status
curl -s http://localhost:8122/api/v1/vms/agent-test | jq .

# 4. Restart VM
RESTART_OP=$(curl -s -X POST http://localhost:8122/api/v1/vms/agent-test:restart \
  -H "Content-Type: application/json" \
  -d '{"mode": "graceful"}' | jq -r '.operation.id')

# 5. Wait for restart
curl -s http://localhost:8122/api/v1/operations/$RESTART_OP | jq .

# 6. Delete VM
curl -X DELETE 'http://localhost:8122/api/v1/vms/agent-test?delete_disk=true&force=true'
```

## Default Values

### Create VM Request
- `profile`: "agentic-dev"
- `vcpus`: 4
- `memory_mb`: 8192
- `disk_gb`: 50
- `agentshare`: true
- `start`: true

### Restart VM Request
- `mode`: "graceful"
- `timeout_seconds`: 60

## VM Name Validation

Valid patterns:
- `agent-01` ✓
- `agent-test` ✓
- `agent-dev-01` ✓
- `agent-a` ✓

Invalid patterns:
- `vm-01` ✗ (must start with "agent-")
- `agent` ✗ (must have suffix)
- `agent-` ✗ (empty suffix)
- `agent-01_test` ✗ (no underscores)
- `AGENT-01` ✗ (uppercase not allowed)


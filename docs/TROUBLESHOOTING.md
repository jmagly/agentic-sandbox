# Troubleshooting Guide

Comprehensive troubleshooting reference for the agentic-sandbox project.

## Quick Diagnostics

### Health Check Commands

```bash
# Management server health
curl http://localhost:8122/healthz

# Detailed health with metrics
curl http://localhost:8122/health

# Check readiness (requires at least one agent)
curl http://localhost:8122/ready

# List connected agents
curl http://localhost:8122/api/v1/agents

# View metrics
curl http://localhost:8122/metrics

# Check specific agent status
curl http://localhost:8122/api/v1/agents/<agent-id>
```

### Status Verification

```bash
# Check VM status
virsh list --all

# Check VM IP and network
virsh domifaddr <vm-name>

# Check if agent service is running inside VM
ssh agent@<vm-ip> systemctl status agentic-agent

# View agent logs
ssh agent@<vm-ip> journalctl -u agentic-agent -f

# Check management server logs
cd management && ./dev.sh logs
```

### Log Locations

| Component | Location | Command |
|-----------|----------|---------|
| Management server | stderr (dev mode) | `./dev.sh logs` |
| Agent service | journald | `journalctl -u agentic-agent -f` |
| VM provisioning | /var/log/cloud-init.log | `ssh agent@<ip> cat /var/log/cloud-init.log` |
| Agentshare output | /mnt/inbox/current | `ls /mnt/inbox/current/` |

## Agent Connection Issues

### Agent Not Connecting

| Symptom | Possible Cause | Solution |
|---------|----------------|----------|
| Agent service inactive | Binary not deployed | Run `./scripts/deploy-agent.sh <vm-name>` |
| "Connection refused" | Management server down | Start server: `cd management && ./dev.sh` |
| "Failed to connect to management server" | Wrong IP in config | Check server IP is 192.168.122.1:8120 |
| "Registration rejected" | Secret mismatch | Verify secret matches between agent and server |
| Agent connects then disconnects | gRPC stream error | Check management server logs for errors |

**Diagnostic Steps:**

```bash
# 1. Verify management server is running
curl http://localhost:8122/healthz
# Expected: "OK"

# 2. Check if VM can reach management server
ssh agent@<vm-ip> ping -c 3 192.168.122.1
# Expected: 0% packet loss

# 3. Test gRPC port connectivity
ssh agent@<vm-ip> nc -zv 192.168.122.1 8120
# Expected: "succeeded!"

# 4. Check agent service status
ssh agent@<vm-ip> systemctl status agentic-agent
# Expected: "active (running)"

# 5. View agent logs for connection errors
ssh agent@<vm-ip> journalctl -u agentic-agent -n 50 --no-pager
```

### "Invalid agent secret" Error

**Root Cause:** The management server stores SHA256 hashes of agent secrets. The agent must send the plaintext secret, not the hash.

| Error Pattern | Cause | Fix |
|---------------|-------|-----|
| "Invalid agent secret" on registration | Using hash instead of plaintext | Deploy agent using scripts (reads plaintext from VM) |
| Secret works initially, fails after restart | Server restarted, hash file missing | Regenerate secret: `./images/qemu/provision-vm.sh <vm-name> --regenerate-secret` |
| Multiple agents share same secret | Secret collision | Each VM needs unique secret (auto-generated during provision) |

**Fix:**

```bash
# Read the plaintext secret from VM
ssh agent@<vm-ip> sudo cat /etc/agentic-sandbox/agent.env | grep AGENT_SECRET

# Verify hash exists on host
grep <agent-id> ~/.config/agentic-sandbox/agent-tokens
# Or check JSON format
cat /var/lib/agentic-sandbox/secrets/agent-hashes.json

# Redeploy with correct secret
./scripts/deploy-agent.sh <vm-name>
```

### Frequent Disconnects

**Symptom:** Agent connects successfully but drops connection every few minutes.

| Possible Cause | Diagnostic | Solution |
|----------------|------------|----------|
| Heartbeat timeout | Check agent logs for "heartbeat" messages | Agent sends heartbeat every 5s, server expects within 60s |
| Network issues | `ping -c 100 192.168.122.1` from VM | Check for packet loss, latency spikes |
| Server overload | Check server CPU/memory | Scale down concurrent VMs or increase resources |
| gRPC keepalive timeout | Check server logs for "keepalive" | Server uses 10s keepalive, 20s timeout (see main.rs:172-174) |

**Fix Heartbeat Issues:**

```bash
# Check heartbeat interval in agent logs
ssh agent@<vm-ip> journalctl -u agentic-agent | grep -i heartbeat

# Verify metrics are being sent
curl http://localhost:8122/api/v1/agents/<agent-id> | jq '.metrics'

# Increase heartbeat interval if network is slow
ssh agent@<vm-ip> sudo systemctl edit agentic-agent
# Add: Environment=HEARTBEAT_INTERVAL=10
```

### Stale Agent Status

**Symptom:** Agent shows as "Stale" or "Disconnected" in dashboard despite being active.

**Lifecycle:**
1. **Ready** - Receiving regular heartbeats
2. **Stale** - No heartbeat for 60 seconds
3. **Disconnected** - Stale for 2 minutes
4. **Removed** - Disconnected for 5 minutes

**Diagnostic:**

```bash
# Check last heartbeat timestamp
curl http://localhost:8122/api/v1/agents/<agent-id> | jq '.last_heartbeat'

# Check agent status
curl http://localhost:8122/api/v1/agents/<agent-id> | jq '.status'

# Monitor heartbeat monitor logs
cd management && ./dev.sh logs | grep -i "stale\|heartbeat"
```

**Fix:**

```bash
# If agent is actually dead, remove it
virsh destroy <vm-name>
# Wait 5 minutes for automatic cleanup, or restart management server

# If agent is alive but marked stale, restart agent
ssh agent@<vm-ip> sudo systemctl restart agentic-agent
```

## VM Issues

### VM Won't Start

| Symptom | Cause | Solution |
|---------|-------|----------|
| `virsh start` fails with "domain not found" | VM not defined | Run provision script: `./images/qemu/provision-vm.sh <vm-name>` |
| "backing file not found" | Base image missing | Build base image: `./images/qemu/build-base-image.sh ubuntu-24.04` |
| "insufficient permissions" | User not in libvirt group | `sudo usermod -aG libvirt $USER && newgrp libvirt` |
| VM starts but shuts down immediately | Cloud-init error | Check `/var/log/cloud-init.log` inside VM |
| "Network not available" | libvirt network down | Start network: `virsh net-start default` |

**Diagnostic Steps:**

```bash
# Check if VM is defined
virsh list --all | grep <vm-name>

# Check VM configuration
virsh dumpxml <vm-name> | less

# Verify base image exists
ls -lh /mnt/ops/base-images/ubuntu-24.04.qcow2

# Check libvirt network
virsh net-list
virsh net-info default

# View VM boot console (blocking command)
virsh console <vm-name>
# Press Ctrl+] to exit
```

### VM Stuck in Provisioning

**Symptom:** VM starts but cloud-init never completes, no SSH access.

**Common Causes:**
- Network not available during cloud-init
- Cloud-init user-data syntax error
- Disk full during setup
- Missing dependencies in profile

**Diagnostic:**

```bash
# Attach to VM console (may require root password)
virsh console <vm-name>

# Check cloud-init status
cloud-init status

# View cloud-init logs
tail -100 /var/log/cloud-init.log
tail -100 /var/log/cloud-init-output.log

# Check if SSH is listening
ss -tlnp | grep :22
```

**Recovery:**

```bash
# Destroy stuck VM
virsh destroy <vm-name>
virsh undefine <vm-name>

# Clean up disk image
rm /var/lib/agentic-sandbox/vms/<vm-name>.qcow2

# Reprovision with minimal profile to test
./images/qemu/provision-vm.sh --profile basic <vm-name>
```

### Cloud-init Failures

**Symptom:** VM boots but cloud-init reports errors, partial configuration.

| Error Pattern | Cause | Fix |
|---------------|-------|-----|
| "failed to download package" | Network timing issue | Add retries to apt commands in profile |
| "permission denied" | File ownership wrong | Ensure files created as correct user (agent) |
| "command not found" | Dependency missing | Add dependency to earlier step in profile |
| "disk full" | Profile too large for disk | Increase disk size: `--disk 60G` |

**Debug cloud-init:**

```bash
# SSH into VM (if SSH works)
ssh agent@<vm-ip>

# Check cloud-init status
cloud-init status --long

# Rerun cloud-init module for testing
sudo cloud-init single --name cc_runcmd --frequency always

# View full cloud-init config
sudo cloud-init query userdata

# Check provisioning script output
cat /var/log/cloud-init-output.log | grep -A 10 -B 5 "ERROR\|FAILED"
```

### Network Not Available

**Symptom:** VM has no network connectivity, can't reach 192.168.122.1.

| Diagnostic | Command | Expected Result |
|------------|---------|-----------------|
| Check IP address | `virsh domifaddr <vm-name>` | Shows 192.168.122.2XX/24 |
| Check from VM | `ssh agent@<ip> ip addr` | Shows IP on eth0/ens3 |
| Ping gateway | `ssh agent@<ip> ping -c 3 192.168.122.1` | 0% packet loss |
| Check DNS | `ssh agent@<ip> ping -c 3 8.8.8.8` | 0% packet loss |

**Fix Network Issues:**

```bash
# Restart libvirt network
virsh net-destroy default
virsh net-start default

# Check network configuration
virsh net-dumpxml default

# Restart VM networking inside VM
ssh agent@<ip> sudo systemctl restart systemd-networkd

# Verify DHCP lease
virsh net-dhcp-leases default
```

### SSH Connection Refused

**Symptom:** `ssh agent@<vm-ip>` fails with "Connection refused".

| Possible Cause | Check | Fix |
|----------------|-------|-----|
| SSH not installed | Check cloud-init-output.log | Add to profile: `apt-get install -y openssh-server` |
| SSH not started | `systemctl status sshd` | `sudo systemctl start ssh` |
| Wrong IP address | Verify IP with `virsh domifaddr` | Use correct IP from libvirt |
| Cloud-init not finished | Check `cloud-init status` | Wait for completion (max 5 minutes) |
| Firewall blocking | Check iptables rules | Disable firewall: `sudo ufw disable` |

**Wait for SSH:**

```bash
# Automated wait script
VM_IP="192.168.122.201"
for i in {1..60}; do
    if ssh -o ConnectTimeout=2 -o StrictHostKeyChecking=no agent@$VM_IP "echo ok" 2>/dev/null; then
        echo "SSH is ready!"
        break
    fi
    echo "Waiting for SSH... ($i/60)"
    sleep 5
done
```

## Task Execution Issues

### Task Stuck in PENDING

**Symptom:** Task created but never starts executing.

| Possible Cause | Diagnostic | Solution |
|----------------|------------|----------|
| No agents available | `curl http://localhost:8122/api/v1/agents` | Start an agent VM |
| Agent doesn't match requirements | Check task requirements | Ensure agent has required profile/labels |
| Dispatcher queue full | Check server metrics | Restart management server |
| Task assigned to disconnected agent | Check agent status | Task will timeout after 5 minutes |

**Diagnostic:**

```bash
# Check task status
curl http://localhost:8122/api/v1/tasks/<task-id>

# List all agents
curl http://localhost:8122/api/v1/agents | jq '.[] | {id: .id, status: .status}'

# Check task queue
curl http://localhost:8122/metrics | grep agentic_tasks_pending

# View dispatcher logs
cd management && ./dev.sh logs | grep dispatcher
```

### Task Stuck in RUNNING

**Symptom:** Task starts but never completes or times out.

| Possible Cause | Check | Fix |
|----------------|-------|-----|
| Long-running process | Check task timeout config | Increase timeout or set to 0 (no timeout) |
| Process hung | SSH to VM, check processes | Kill process: `pkill -9 <name>` |
| Output not streaming | Check agent logs | Agent may have crashed |
| Hang detection not triggered | Check hang detection config | Threshold may be too high (default 10 min) |

**Diagnostic:**

```bash
# Get task details
curl http://localhost:8122/api/v1/tasks/<task-id> | jq '.'

# Check command ID from task
COMMAND_ID=$(curl -s http://localhost:8122/api/v1/tasks/<task-id> | jq -r '.command_id')

# SSH to agent and check process
ssh agent@<vm-ip> ps aux | grep -v grep | grep <process>

# Check for zombie processes
ssh agent@<vm-ip> ps aux | grep Z

# Force kill if needed
ssh agent@<vm-ip> sudo pkill -9 -f <command>
```

### Hang Detection Triggered

**Symptom:** Task automatically terminated with "hang detected" message.

**Hang Detection Strategies:**
- **Output Silence**: No output for 10 minutes (default)
- **CPU Idle**: CPU < 5% for 15 minutes
- **Process Stuck**: No progress indicators for 20 minutes

**Configuration:**

```bash
# Check hang detection settings
curl http://localhost:8122/api/v1/tasks/<task-id>/hang-config

# Disable hang detection for long-running tasks
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "command": "long-running-script.sh",
    "hang_detection": {
      "enabled_strategies": [],
      "recovery_action": "notify_only"
    }
  }'
```

**Override for Specific Task:**

```rust
// In task manifest YAML
hang_detection:
  output_silence_threshold_minutes: 30
  cpu_idle_threshold_minutes: 60
  recovery_action: preserve_for_debug
```

### Artifact Collection Failed

**Symptom:** Task completes but artifacts are missing or incomplete.

| Error Pattern | Cause | Solution |
|---------------|-------|----------|
| "artifact path not found" | Path doesn't exist in VM | Verify path is correct, file was created |
| "permission denied" | File not readable by agent user | Change ownership: `chown agent:agent <file>` |
| "tar: file changed" | File modified during collection | Wait for process to finish before collecting |
| Artifact size is 0 | Glob pattern matched nothing | Check pattern syntax, use absolute paths |

**Manual Collection:**

```bash
# SSH into VM
ssh agent@<vm-ip>

# Check if file exists
ls -lh /path/to/artifact

# Check permissions
stat /path/to/artifact

# Manually collect via scp
scp -r agent@<vm-ip>:/path/to/artifact ./local-artifacts/

# Check agentshare inbox
ls /mnt/inbox/<agent-id>/runs/*/outputs/
```

### Timeout Exceeded

**Symptom:** Task killed after timeout period.

**Default Timeouts:**
- Command timeout: Set per-command (0 = no timeout)
- Task timeout: 1 hour default
- Hang detection: 10 minutes output silence

**Increase Timeout:**

```bash
# For command execution
curl -X POST http://localhost:8122/api/v1/tasks \
  -H "Content-Type: application/json" \
  -d '{
    "command": "slow-build.sh",
    "timeout_seconds": 7200
  }'

# For orchestrated tasks (in manifest)
timeout_seconds: 7200
```

## Session Issues

### Session Not Responding

**Symptom:** PTY session created but not responding to input.

| Diagnostic | Command | Expected |
|------------|---------|----------|
| Check session exists | `curl http://localhost:8122/api/v1/sessions` | Session listed with status ACTIVE |
| Check from agent | `ssh agent@<ip> ps aux \| grep tmux` | Process running |
| Test stdin | Send test input via WebSocket | Should echo back |

**Fix:**

```bash
# Get session ID
SESSION_ID=$(curl -s http://localhost:8122/api/v1/sessions | jq -r '.sessions[0].id')

# Check session details
curl http://localhost:8122/api/v1/sessions/$SESSION_ID

# Send test input
wscat -c ws://localhost:8121/sessions/$SESSION_ID/attach
# Type: echo test

# If unresponsive, kill session
curl -X DELETE http://localhost:8122/api/v1/sessions/$SESSION_ID
```

### PTY Not Working

**Symptom:** Session created but terminal features don't work (colors, cursor movement).

| Problem | Cause | Fix |
|---------|-------|-----|
| No colors | TERM not set | Set `pty_term: "xterm-256color"` |
| Wrong dimensions | Terminal size not sent | Send resize: `pty_cols: 80, pty_rows: 24` |
| Broken cursor | Line discipline issue | Restart session with fresh PTY |
| Echo disabled | Stdin/stdout mixed up | Check WebSocket frame types |

**Create Proper PTY Session:**

```bash
curl -X POST http://localhost:8122/api/v1/sessions \
  -H "Content-Type: application/json" \
  -d '{
    "agent_id": "agent-01",
    "session_name": "debug",
    "command": "/bin/bash",
    "allocate_pty": true,
    "pty_term": "xterm-256color",
    "pty_cols": 120,
    "pty_rows": 30
  }'
```

### Session Orphaned After Restart

**Symptom:** After management server restart, PTY sessions still running on agent but not tracked by server.

**Root Cause:** Server state is not persisted. Session reconciliation handles cleanup.

**Automatic Cleanup (Implemented):**

When agent reconnects after server restart:
1. Server sends `SessionQuery` to agent
2. Agent reports all running sessions
3. Server sends `SessionReconcile` with sessions to kill
4. Agent terminates orphaned sessions

**Manual Cleanup:**

```bash
# List orphaned sessions on agent
ssh agent@<vm-ip> tmux list-sessions

# Kill specific session
ssh agent@<vm-ip> tmux kill-session -t main

# Kill all tmux sessions
ssh agent@<vm-ip> pkill -TERM tmux

# Check running_commands map
ssh agent@<vm-ip> journalctl -u agentic-agent -n 100 | grep "running_commands"
```

### Reconciliation Failures

**Symptom:** Session reconciliation errors in logs, sessions not cleaned up.

| Error Pattern | Cause | Fix |
|---------------|-------|-----|
| "Failed to kill session" | Process already dead | Ignore (harmless) |
| "Session not found" | Race with process exit | Ignore (already cleaned) |
| "Permission denied" | Agent can't kill root process | Don't run sessions as root |
| "Reconcile timeout" | Agent didn't respond | Check agent connectivity |

**Debug Reconciliation:**

```bash
# View reconciliation logs on server
cd management && ./dev.sh logs | grep -i reconcil

# View reconciliation on agent
ssh agent@<vm-ip> journalctl -u agentic-agent | grep -i reconcil

# Check for zombie sessions
ssh agent@<vm-ip> ps aux | grep Z

# Force clean state
ssh agent@<vm-ip> sudo pkill -KILL tmux
ssh agent@<vm-ip> sudo systemctl restart agentic-agent
```

## Performance Issues

### High CPU Usage

| Component | Normal Range | High Threshold | Investigation |
|-----------|--------------|----------------|---------------|
| Management server | 5-15% | > 50% | Check active task count, gRPC connections |
| Agent | 5-20% | > 80% | Check running commands, metrics collection frequency |
| VM hypervisor (qemu) | 10-30% per VM | > 60% per VM | Check workload, reduce concurrent VMs |

**Diagnostic:**

```bash
# Server CPU usage
top -p $(pgrep agentic-management)

# Check active tasks
curl http://localhost:8122/metrics | grep agentic_tasks_running

# Agent CPU usage
ssh agent@<vm-ip> top -b -n 1

# Check metrics collection frequency
ssh agent@<vm-ip> journalctl -u agentic-agent | grep -i metrics | tail -20
```

**Fix High CPU:**

```bash
# Reduce heartbeat frequency (agent)
ssh agent@<vm-ip> sudo systemctl edit agentic-agent
# Add: Environment=HEARTBEAT_INTERVAL=10

# Reduce metrics collection frequency (server)
# Edit management/src/telemetry/metrics.rs, increase interval

# Limit concurrent VMs
# Max recommended: 4-6 VMs on 32GB host
```

### Memory Exhaustion

**Symptoms:**
- OOMKiller messages in dmesg
- Slow response times
- VMs failing to start

**Memory Allocation:**

| Component | Typical Usage | Limit |
|-----------|---------------|-------|
| Management server | 200-500 MB | 2 GB |
| Agent process | 50-100 MB | 1 GB |
| VM (agentic-dev) | 6-8 GB | 8 GB (default) |
| VM (basic) | 1-2 GB | 4 GB |

**Diagnostic:**

```bash
# Check host memory
free -h
dmesg | grep -i oom

# Check VM memory limits
virsh dominfo <vm-name> | grep memory

# Monitor memory usage
watch -n 1 'free -h; echo; virsh list --all | grep running'
```

**Fix Memory Issues:**

```bash
# Reduce VM memory allocation
virsh setmem <vm-name> 4G --config
virsh shutdown <vm-name>
virsh start <vm-name>

# Or reprovision with lower memory
./images/qemu/provision-vm.sh --memory 4G <vm-name>

# Enable memory ballooning (dynamic allocation)
virsh dumpxml <vm-name> | grep balloon
# If not present, add memballoon device to VM XML
```

### Disk Space Full

**Common Locations:**

| Path | Usage | Cleanup Strategy |
|------|-------|------------------|
| `/var/lib/agentic-sandbox/vms/` | VM disk images | Delete unused VMs |
| `/mnt/ops/base-images/` | Base images | Keep only latest 2 versions |
| `/srv/agentshare/tasks/` | Task artifacts | Archive old tasks, set retention policy |
| `/srv/agentshare/inbox/` | Agent outputs | Implement log rotation |

**Diagnostic:**

```bash
# Check disk usage
df -h

# Find large directories
du -h --max-depth=1 /var/lib/agentic-sandbox/ | sort -h

# Find large files
find /srv/agentshare -type f -size +100M -exec ls -lh {} \;

# Check VM disk usage
ssh agent@<vm-ip> df -h
```

**Cleanup:**

```bash
# Delete stopped VMs
for vm in $(virsh list --all --name | grep -v running); do
    virsh undefine $vm
    rm /var/lib/agentic-sandbox/vms/${vm}.qcow2
done

# Clean old task artifacts (older than 7 days)
find /srv/agentshare/tasks -type d -mtime +7 -exec rm -rf {} \;

# Clean old inbox runs (keep last 10 per agent)
for agent_dir in /srv/agentshare/inbox/*/runs; do
    ls -t $agent_dir | tail -n +11 | xargs -I {} rm -rf $agent_dir/{}
done

# Compact qcow2 images
qemu-img convert -O qcow2 old.qcow2 new.qcow2
mv new.qcow2 old.qcow2
```

### Network Bottlenecks

**Symptoms:**
- Slow artifact uploads
- WebSocket frame drops
- gRPC timeout errors

**Diagnostic:**

```bash
# Check network throughput
iftop -i virbr0

# Test transfer speed to VM
ssh agent@<vm-ip> "dd if=/dev/zero bs=1M count=100" | pv > /dev/null

# Check network latency
ping -c 100 192.168.122.201 | tail -1

# Monitor gRPC traffic
cd management && ./dev.sh logs | grep -i grpc
```

**Optimization:**

```bash
# Use virtiofs instead of 9p for agentshare (faster)
# Already implemented in provision-vm.sh

# Reduce output streaming frequency
# Batch stdout/stderr chunks on agent side

# Use compression for artifact collection
ssh agent@<vm-ip> tar czf - /path/to/artifacts | pv > artifacts.tar.gz
```

## Management Server Issues

### Server Won't Start

| Error Message | Cause | Solution |
|---------------|-------|----------|
| "Address already in use" | Port 8120/8121/8122 in use | Kill existing process: `pkill agentic-management` |
| "Permission denied" | Can't bind to privileged port | Use ports > 1024 or run with sudo (not recommended) |
| "No such file or directory" | Binary not built | `cd management && cargo build --release` |
| "Secrets directory not found" | Missing config | Create: `mkdir -p ~/.config/agentic-sandbox` |
| libvirt connection failed | libvirt not running | `sudo systemctl start libvirtd` |

**Startup Checklist:**

```bash
# 1. Check if process already running
pgrep agentic-management
# If found: kill -9 <pid>

# 2. Check port availability
ss -tlnp | grep -E "8120|8121|8122"

# 3. Verify secrets directory
ls -la ~/.config/agentic-sandbox/

# 4. Start server in dev mode
cd management
./dev.sh
# Watch for errors on startup

# 5. Test endpoints
curl http://localhost:8122/healthz
```

### Port Already in Use

**Symptom:** Server fails to start with "bind: address already in use".

**Find and Kill:**

```bash
# Find process using port 8122 (HTTP)
sudo lsof -i :8122
# Or
ss -tlnp | grep 8122

# Kill the process
kill -9 <PID>

# For all server ports
for port in 8120 8121 8122; do
    sudo lsof -ti :$port | xargs -r kill -9
done
```

**Change Ports (if needed):**

```bash
# Edit management/src/main.rs
# Change DEFAULT_LISTEN_ADDR = "0.0.0.0:9120"

# Or use environment variable
LISTEN_ADDR="0.0.0.0:9120" ./dev.sh
```

### gRPC Connection Refused

**Symptom:** Agent logs show "Failed to connect to management server".

| Check | Command | Fix |
|-------|---------|-----|
| Server running | `curl localhost:8122/healthz` | Start server |
| Port listening | `ss -tlnp \| grep 8120` | Check bind address |
| Firewall | `sudo iptables -L \| grep 8120` | Allow port |
| TLS issues | Check for certificate errors | We use plaintext HTTP/2 (h2c) |

**Test gRPC Connectivity:**

```bash
# From host
grpcurl -plaintext localhost:8120 list

# From VM
ssh agent@<vm-ip> nc -zv 192.168.122.1 8120
```

### WebSocket Not Connecting

**Symptom:** Dashboard shows "WebSocket disconnected", can't stream output.

**WebSocket Port:** 8121 (HTTP port + 1)

**Diagnostic:**

```bash
# Test WebSocket connectivity
wscat -c ws://localhost:8121/health
# Expected: connection established

# Check if WebSocket server started
curl http://localhost:8122/healthz
# Then check logs for "Starting WebSocket server"

# Test from browser console
ws = new WebSocket("ws://localhost:8121/streams/test-123");
ws.onopen = () => console.log("Connected");
```

**Common Fixes:**

```bash
# 1. Wrong URL (use ws:// not http://)
# Correct: ws://localhost:8121/streams/<stream-id>
# Wrong: http://localhost:8121/streams/<stream-id>

# 2. Port blocked by firewall
sudo firewall-cmd --add-port=8121/tcp --permanent
sudo firewall-cmd --reload

# 3. WebSocket hub crashed
cd management && ./dev.sh restart
```

## Storage Issues

### virtiofs Mount Failed

**Symptom:** VM starts but `/mnt/inbox` or `/mnt/global` are empty.

| Error in VM | Cause | Fix |
|-------------|-------|-----|
| "mount: unknown filesystem type virtiofs" | Kernel too old | Ubuntu 22.04+ required |
| "mount.virtiofs: Connection refused" | virtiofsd not running | Check VM XML, restart VM |
| "Permission denied" | SELinux/AppArmor | Disable or configure policy |

**Diagnostic:**

```bash
# Check if mount succeeded
ssh agent@<vm-ip> mount | grep virtiofs

# Check mount points
ssh agent@<vm-ip> ls /mnt/inbox /mnt/global

# Test writing to inbox
ssh agent@<vm-ip> touch /mnt/inbox/test.txt

# Check virtiofs in VM config
virsh dumpxml <vm-name> | grep virtiofs
```

**Fix Mount Issues:**

```bash
# Ensure agentshare directory exists on host
sudo mkdir -p /srv/agentshare/inbox/<agent-id>
sudo mkdir -p /srv/agentshare/global
sudo chown -R $USER:$USER /srv/agentshare

# Reprovision VM with agentshare
./images/qemu/provision-vm.sh --agentshare <vm-name>

# Manual mount inside VM (for testing)
ssh agent@<vm-ip> sudo mount -t virtiofs agentshare_inbox /mnt/inbox
```

### Permission Denied

**Symptom:** Agent can't write to `/mnt/inbox`, gets "Permission denied".

**virtiofs Permissions:**
- Mount tag must have correct permissions on host
- Files created inside VM inherit host UID/GID
- Recommended: Use UID=1000 (agent user) on both host and VM

**Fix:**

```bash
# Check ownership on host
ls -la /srv/agentshare/inbox/<agent-id>/

# Fix ownership (host side)
sudo chown -R $USER:$USER /srv/agentshare/

# Check ownership inside VM
ssh agent@<vm-ip> ls -la /mnt/inbox/

# Fix ownership (VM side)
ssh agent@<vm-ip> sudo chown -R agent:agent /mnt/inbox/
```

### Disk Quota Exceeded

**Symptom:** "No space left on device" but `df -h` shows space available.

**Cause:** Disk quota enforced per VM (default: 40GB).

**Check Quota:**

```bash
# Inside VM
ssh agent@<vm-ip> df -h

# Check qcow2 image size
qemu-img info /var/lib/agentic-sandbox/vms/<vm-name>.qcow2

# Check actual disk usage
du -sh /var/lib/agentic-sandbox/vms/<vm-name>.qcow2
```

**Increase Quota:**

```bash
# Stop VM
virsh shutdown <vm-name>

# Resize qcow2 image
qemu-img resize /var/lib/agentic-sandbox/vms/<vm-name>.qcow2 +20G

# Start VM and resize filesystem
virsh start <vm-name>
ssh agent@<vm-ip> sudo growpart /dev/vda 1
ssh agent@<vm-ip> sudo resize2fs /dev/vda1

# Verify new size
ssh agent@<vm-ip> df -h /
```

### Inbox Not Writable

**Symptom:** Agent can read from `/mnt/inbox` but can't create files.

**Diagnostic:**

```bash
# Test write access
ssh agent@<vm-ip> touch /mnt/inbox/writetest.txt
# Expected: No error

# Check mount options
ssh agent@<vm-ip> mount | grep inbox
# Should NOT show "ro" (read-only)

# Check permissions
ssh agent@<vm-ip> stat /mnt/inbox
```

**Fix:**

```bash
# Remount as read-write (inside VM)
ssh agent@<vm-ip> sudo mount -o remount,rw /mnt/inbox

# Or reprovision with correct mount flags
# Edit images/qemu/provision-vm.sh
# Ensure: rw,relatime in fstab entry
```

## Security Issues

### Secret Rotation Needed

**When to Rotate:**
- Agent secret compromised
- Periodic security policy (every 90 days)
- Agent migration to new VM

**Rotation Process:**

```bash
# 1. Generate new secret
./images/qemu/provision-vm.sh <vm-name> --regenerate-secret

# 2. The script will:
#    - Generate new plaintext secret
#    - Update hash in agent-tokens and agent-hashes.json
#    - Update /etc/agentic-sandbox/agent.env in VM
#    - Restart agent service

# 3. Verify new secret working
ssh agent@<vm-ip> journalctl -u agentic-agent -n 10
# Should show successful registration

# 4. Old secret automatically invalidated
```

**Manual Rotation:**

```bash
# Generate new secret
NEW_SECRET=$(openssl rand -hex 32)

# Update on VM
ssh agent@<vm-ip> sudo tee /etc/agentic-sandbox/agent.env > /dev/null <<EOF
AGENT_ID=<agent-id>
AGENT_SECRET=$NEW_SECRET
MANAGEMENT_SERVER=192.168.122.1:8120
EOF

# Update hash on host
NEW_HASH=$(echo -n "$NEW_SECRET" | sha256sum | cut -d' ' -f1)
grep -v "^<agent-id>:" ~/.config/agentic-sandbox/agent-tokens > /tmp/tokens
echo "<agent-id>:$NEW_HASH" >> /tmp/tokens
sudo mv /tmp/tokens ~/.config/agentic-sandbox/agent-tokens

# Update JSON format (for management server)
python3 -c "
import json
with open('/var/lib/agentic-sandbox/secrets/agent-hashes.json') as f:
    data = json.load(f)
data['<agent-id>'] = '$NEW_HASH'
with open('/var/lib/agentic-sandbox/secrets/agent-hashes.json', 'w') as f:
    json.dump(data, f, indent=2)
"

# Restart agent
ssh agent@<vm-ip> sudo systemctl restart agentic-agent
```

### Certificate Errors

**Note:** This system uses plaintext gRPC (h2c), not TLS.

If you see certificate errors, check:

```bash
# Verify connection is NOT using TLS
cd management && ./dev.sh logs | grep -i tls
# Should be empty

# Check agent connection string
ssh agent@<vm-ip> journalctl -u agentic-agent | grep "Connecting to"
# Should show: "host.internal:8120" or "192.168.122.1:8120"
# NOT: "https://" or "grpcs://"
```

### Authentication Failures

**Symptom:** Agent rejected with "authentication failed".

| Error Pattern | Diagnostic | Fix |
|---------------|------------|-----|
| "Invalid agent secret" | Secret mismatch | Redeploy agent with correct secret |
| "Agent ID not registered" | Agent not in registry | Check agent-tokens file |
| "Secret hash mismatch" | Hash algorithm changed | Regenerate secret |
| "Authentication required" | Missing auth metadata | Check gRPC metadata headers |

**Verify Authentication:**

```bash
# Check agent sends correct metadata
ssh agent@<vm-ip> journalctl -u agentic-agent | grep "x-agent-id\|x-agent-secret"

# Check server validates correctly
cd management && ./dev.sh logs | grep "authentication\|registration"

# Test authentication manually
# (Requires grpcurl with auth headers)
grpcurl -plaintext \
  -H "x-agent-id: agent-01" \
  -H "x-agent-secret: <secret>" \
  localhost:8120 agentic.sandbox.v1.AgentService/Connect
```

## Diagnostic Commands Reference

### Quick Health Check

```bash
#!/bin/bash
# comprehensive-health-check.sh

echo "=== Management Server ==="
curl -s http://localhost:8122/healthz && echo " (OK)" || echo " (FAILED)"

echo ""
echo "=== Connected Agents ==="
curl -s http://localhost:8122/api/v1/agents | jq -r '.[] | "\(.id): \(.status)"'

echo ""
echo "=== VMs Running ==="
virsh list --name

echo ""
echo "=== Agent Services ==="
for vm in $(virsh list --name); do
    IP=$(virsh domifaddr $vm 2>/dev/null | awk '/ipv4/ {print $4}' | cut -d/ -f1)
    if [ -n "$IP" ]; then
        STATUS=$(ssh -o ConnectTimeout=2 agent@$IP systemctl is-active agentic-agent 2>/dev/null)
        echo "$vm ($IP): $STATUS"
    fi
done

echo ""
echo "=== Disk Space ==="
df -h | grep -E "/$|/var|/srv"

echo ""
echo "=== Memory Usage ==="
free -h
```

### Collect Debug Bundle

```bash
#!/bin/bash
# collect-debug-bundle.sh <agent-id>

AGENT_ID=$1
OUTPUT_DIR="debug-bundle-$(date +%Y%m%d-%H%M%S)"

mkdir -p $OUTPUT_DIR

# Server logs
cd management && cargo run > $OUTPUT_DIR/server.log 2>&1 &
sleep 5
kill $!

# Agent logs
VM_IP=$(virsh domifaddr $AGENT_ID | awk '/ipv4/ {print $4}' | cut -d/ -f1)
ssh agent@$VM_IP journalctl -u agentic-agent -n 500 > $OUTPUT_DIR/agent.log

# Configuration
virsh dumpxml $AGENT_ID > $OUTPUT_DIR/vm-config.xml
cp ~/.config/agentic-sandbox/agent-tokens $OUTPUT_DIR/
ssh agent@$VM_IP cat /etc/agentic-sandbox/agent.env > $OUTPUT_DIR/agent.env

# System info
curl -s http://localhost:8122/api/v1/agents/$AGENT_ID > $OUTPUT_DIR/agent-status.json
ssh agent@$VM_IP "df -h; free -h; ps aux" > $OUTPUT_DIR/system-info.txt

# Create tarball
tar czf debug-bundle.tar.gz $OUTPUT_DIR
rm -rf $OUTPUT_DIR
echo "Debug bundle: debug-bundle.tar.gz"
```

## Log Analysis

### Common Error Patterns

| Log Pattern | Meaning | Action |
|-------------|---------|--------|
| `Invalid agent secret` | Authentication failure | Check secret matches between VM and host |
| `Connection refused` | Can't reach server | Verify server running, network OK |
| `Heartbeat timeout` | Agent stopped sending heartbeats | Check agent process, network latency |
| `Task timeout exceeded` | Task ran too long | Increase timeout or optimize task |
| `Hang detected: output silence` | No output for 10+ minutes | Check if process is actually hung |
| `Failed to collect artifact` | Artifact path not found | Verify path exists, permissions OK |
| `virtiofs mount failed` | Agentshare not mounted | Check virtiofsd, remount filesystem |
| `Session reconciliation failed` | Can't clean up orphaned session | Manual cleanup with pkill |

### Correlation with Trace IDs

**Trace IDs** are used to correlate logs across components.

Example workflow:

```bash
# User creates task, receives task ID
TASK_ID="task-abc-123"

# Find associated command ID
COMMAND_ID=$(curl -s http://localhost:8122/api/v1/tasks/$TASK_ID | jq -r '.command_id')

# Search server logs for command
cd management && ./dev.sh logs | grep $COMMAND_ID

# Search agent logs for command
ssh agent@<vm-ip> journalctl -u agentic-agent | grep $COMMAND_ID

# Find related gRPC stream messages
cd management && ./dev.sh logs | grep "stream_id=$COMMAND_ID"
```

## Recovery Procedures

### Restart Services

```bash
# Restart management server
cd management && ./dev.sh restart

# Restart single agent
ssh agent@<vm-ip> sudo systemctl restart agentic-agent

# Restart all agents
for vm in $(virsh list --name); do
    IP=$(virsh domifaddr $vm | awk '/ipv4/ {print $4}' | cut -d/ -f1)
    [ -n "$IP" ] && ssh agent@$IP sudo systemctl restart agentic-agent
done
```

### Force-Kill Sessions

```bash
# Kill specific session by command ID
COMMAND_ID="cmd-abc-123"
curl -X POST http://localhost:8122/api/v1/sessions/$COMMAND_ID/kill

# Kill all sessions on agent
ssh agent@<vm-ip> sudo pkill -KILL tmux

# Clean up running_commands map (agent restart)
ssh agent@<vm-ip> sudo systemctl restart agentic-agent
```

### Reprovision VM

```bash
# Full reprovision (destructive)
./scripts/destroy-vm.sh <vm-name>
./images/qemu/provision-vm.sh --profile agentic-dev --agentshare <vm-name>
./scripts/deploy-agent.sh <vm-name>

# Or use reprovision script (preserves name/IP)
./scripts/reprovision-vm.sh <vm-name> --profile agentic-dev
```

### Clear Stuck Tasks

```bash
# List stuck tasks
curl http://localhost:8122/api/v1/tasks | jq '.[] | select(.status=="RUNNING")'

# Cancel task
curl -X POST http://localhost:8122/api/v1/tasks/<task-id>/cancel

# If cancel doesn't work, restart management server
cd management && ./dev.sh restart
```

## AIWG Serve Integration Issues

### Sandbox Not Registering

**Symptom:** No `Registered with aiwg serve` log line after startup.

```bash
# Check the endpoint is reachable
curl http://<AIWG_SERVE_ENDPOINT>/api/health

# Check the env var is set
./dev.sh logs | head -20
# Should show: aiwg serve not reachable at ... (will retry every 5 s)
# This is normal if aiwg serve hasn't started yet — registration retries automatically
```

The server retries registration every 5 seconds indefinitely. Start aiwg serve at any point and registration will complete on the next attempt.

### Events Not Flowing to Dashboard

**Symptom:** Sandbox is registered but no agent/session events appear in aiwg serve dashboard.

```bash
# Check WebSocket connection in logs
./dev.sh logs | grep -i "aiwg serve WS"
# Should show: aiwg serve WS connected: ws://...

# If it shows repeated reconnects, check aiwg serve logs
# aiwg serve may be rejecting the token

# Verify sandbox token is valid
curl http://<AIWG_SERVE_ENDPOINT>/api/sandboxes/<sandbox-id> \
  -H "Authorization: Bearer <token>"
```

### WebSocket Authentication Failure

**Symptom:** `aiwg serve WS` connection attempts fail immediately.

The management server receives a token from the registration response and passes it as `?token=<token>` on the WebSocket URL. If registration succeeded but WS fails:

1. Check aiwg serve logs for auth errors
2. Verify the management server time is in sync with aiwg serve (token expiry)
3. Restart the management server to force re-registration and a fresh token

### HITL Requests Not Appearing in aiwg serve

**Symptom:** HITL requests appear in `GET /api/v1/hitl` locally but not in the aiwg serve dashboard.

```bash
# Confirm aiwg serve handle is wired in (check startup logs)
./dev.sh logs | grep -i "aiwg"

# Verify the hitl event type is listed in serve-guide event schema
# HITL events require aiwg serve v2026.4.0+
curl http://<AIWG_SERVE_ENDPOINT>/api/version
```

### Standalone Mode Not Working After Removing AIWG Config

If you remove `AIWG_SERVE_ENDPOINT` and the server behaves unexpectedly:

```bash
# Confirm variable is gone from all config sources
./dev.sh stop
grep -r AIWG_SERVE management/.run/
./dev.sh
```

---

## Getting Help

### Diagnostic Information to Collect

When reporting issues, include:

1. **Environment:**
   ```bash
   uname -a
   cat /etc/os-release
   virsh version
   cargo --version
   ```

2. **Server state:**
   ```bash
   curl http://localhost:8122/health | jq
   curl http://localhost:8122/api/v1/agents | jq
   ```

3. **Agent state:**
   ```bash
   ssh agent@<vm-ip> systemctl status agentic-agent
   ssh agent@<vm-ip> journalctl -u agentic-agent -n 100 --no-pager
   ```

4. **Logs:**
   - Last 100 lines from management server
   - Last 100 lines from agent service
   - cloud-init logs if provisioning issue

### Issue Tracker

Report issues at: https://git.integrolabs.net/roctinam/agentic-sandbox/issues

**Issue Template:**

```markdown
## Environment

- OS: Ubuntu 24.04
- Rust version: 1.78.0
- Libvirt version: 10.0.0

## Reproduction Steps

1. Start management server
2. Provision VM with agentic-dev profile
3. Create PTY session
4. Observe error

## Expected Behavior

Session should remain responsive

## Actual Behavior

Session hangs after 5 minutes

## Logs

[Attach debug bundle or paste relevant logs]

## Additional Context

This started happening after upgrading libvirt
```

## Appendix: Default Configuration

### Management Server

| Setting | Default | Environment Variable |
|---------|---------|---------------------|
| gRPC address | 0.0.0.0:8120 | LISTEN_ADDR |
| WebSocket port | 8121 | (LISTEN_PORT + 1) |
| HTTP port | 8122 | (LISTEN_PORT + 2) |
| Secrets directory | ~/.config/agentic-sandbox | SECRETS_DIR |
| Log level | info | RUST_LOG |
| Heartbeat timeout | 60 seconds | (hardcoded) |
| Stale cleanup | 300 seconds | (hardcoded) |

### Agent

| Setting | Default | Environment Variable |
|---------|---------|---------------------|
| Server address | host.internal:8120 | MANAGEMENT_SERVER |
| Heartbeat interval | 5 seconds | HEARTBEAT_INTERVAL |
| Log level | info | RUST_LOG |
| Log format | pretty | LOG_FORMAT |
| Reconnect delay | 5 seconds | (hardcoded) |
| Max reconnect delay | 60 seconds | (hardcoded) |

### VM Resources

| Profile | CPUs | Memory | Disk | Boot Time |
|---------|------|--------|------|-----------|
| basic | 2 | 2GB | 20GB | ~30s |
| agentic-dev | 4 | 8GB | 40GB | ~60s |

### Timeouts

| Operation | Default | Configurable |
|-----------|---------|--------------|
| Command execution | No timeout (0) | Per-command |
| Task execution | 1 hour | Per-task |
| Hang detection (output silence) | 10 minutes | Per-task |
| Hang detection (CPU idle) | 15 minutes | Per-task |
| Hang detection (process stuck) | 20 minutes | Per-task |
| Session reconciliation | 30 seconds | (hardcoded) |
| Session kill grace period | 5 seconds | Per-reconcile |

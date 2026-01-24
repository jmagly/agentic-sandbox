# UC-005: Persistent Workspace Across Sessions

## Use Case Overview

**ID**: UC-005
**Priority**: High
**Status**: Implemented (Docker volumes, needs testing for VM persistence)
**Last Updated**: 2026-01-05

## Summary

Developer configures agent sandbox with persistent workspace volume, enabling agent work to survive container restarts, crashes, or manual stops. Agent can resume tasks from saved state across multiple sessions.

## Actors

**Primary**: Developer
**Secondary**: Agent (inside sandbox)
**Supporting**: Docker Volume Manager, libvirt Disk Manager

## Stakeholders and Interests

- **Developer**: Wants agent work preserved across sessions, no data loss on failures
- **Agent**: Needs consistent state on resume, access to prior work artifacts
- **Operations**: Requires predictable storage lifecycle, backup/restore capability
- **Data Integrity**: Needs assurance of no corruption during container stop/restart

## Preconditions

- Workspace volume configured in agent YAML or launch command
- Host directory exists with appropriate permissions (if bind mount)
- Docker volume exists (if named volume) or will be created automatically
- Agent writes outputs to /workspace inside container
- Sufficient host disk space for workspace data

## Postconditions

**Success**:
- Workspace data persisted to host filesystem or Docker volume
- Container stopped/destroyed without data loss
- New container launched with same workspace mount
- Agent resumes from saved state (partial outputs, checkpoints, logs)

**Failure**:
- Workspace mount fails with clear error (permission denied, directory not found)
- Container launch aborted if mount fails (no partial state)

## Main Success Scenario

1. Developer launches agent with workspace mount:
   ```bash
   ./scripts/sandbox-launch.sh --runtime docker --image agent-claude \
     --mount ./workspace:/workspace --detach --task "Refactor codebase"
   ```
2. Sandbox launcher creates Docker container with volume mount
3. Host directory ./workspace mounted to /workspace inside container (read-write)
4. Agent starts task, clones repository to /workspace/repo
5. Agent makes progress: refactors 50 files, commits changes to local git
6. Agent saves checkpoint file: /workspace/.checkpoint/progress.json
7. Container crashes due to memory leak (OOM kill)
8. Host workspace directory preserved: ./workspace/repo, ./workspace/.checkpoint intact
9. Developer relaunches agent with same mount:
   ```bash
   ./scripts/sandbox-launch.sh --runtime docker --image agent-claude \
     --mount ./workspace:/workspace --detach --task "Resume refactoring"
   ```
10. New container mounts same workspace directory
11. Agent detects checkpoint file, resumes from 50 files completed
12. Agent continues refactoring remaining files
13. Task completes, final outputs in ./workspace/repo
14. Developer retrieves completed work from host filesystem

**Expected Duration**: Workspace mount <1s, data persistence immediate

## Alternative Flows

**1a. Developer uses named Docker volume instead of bind mount**:
- Launch with: `--mount refactor-workspace:/workspace`
- Docker creates managed volume if not exists
- Data persisted in Docker volume storage (/var/lib/docker/volumes/)
- Developer retrieves via: `docker cp <container>:/workspace ./output`

**3a. Workspace directory permissions incorrect (not writable)**:
- Container starts but agent cannot write to /workspace
- Agent receives "Permission denied" error on write attempt
- Container logs show permission error
- Developer fixes permissions, relaunches

**7a. Container stopped gracefully (manual docker stop)**:
- Agent receives SIGTERM, graceful shutdown
- Agent saves final checkpoint before exit
- Workspace fully synced to host (no data loss)

**9a. Developer launches different agent image with same workspace**:
- New agent has different tooling (Python vs Node.js)
- Workspace data compatible (plain text, git repos)
- Agent continues work with different tools

**13a. Workspace data too large for host disk**:
- Agent writes 100GB, exceeds disk quota
- Disk full error, agent cannot write more
- Existing workspace data preserved (read-only)
- Developer cleans up or expands disk

## Exception Flows

**E1. Workspace corruption during container crash**:
- Agent writing file, container killed mid-write
- Partial file written, potentially corrupted
- Agent on resume detects corruption (checksum validation)
- Agent rolls back to last valid checkpoint

**E2. Multiple containers mount same workspace simultaneously**:
- Developer accidentally launches two agents with same mount
- Concurrent writes cause file conflicts
- Git operations may fail (lock files conflict)
- System warns: "Workspace already mounted by container <id>"

**E3. Host filesystem fills during agent execution**:
- Agent writes logs, fills host disk completely
- Container cannot write more data
- Workspace read-only from agent perspective
- Host system may become unstable (systemd logs fail)

**E4. Workspace mount path conflict (container vs host)**:
- Agent expects /workspace, launch specifies /data
- Agent writes to /data, not found on resume
- Developer must use consistent mount paths

## Business Rules

**BR-001**: Workspace path inside container always /workspace (standard location)
**BR-002**: Bind mounts require absolute host paths (no relative paths)
**BR-003**: Named volumes preferred for production (managed lifecycle)
**BR-004**: Workspace data retention: 30 days unless manually cleaned
**BR-005**: Maximum workspace size: 500GB per agent (disk quota enforced)

## Special Requirements

### Performance
- Mount overhead: <100ms added to container launch time
- I/O performance: Near-native disk speeds (bind mounts on NVMe SSD)
- Sync latency: Writes visible on host within 1 second (filesystem cache)

### Reliability
- Data persistence: 100% of workspace data survives container stop/restart
- Crash resistance: No data loss on OOM kill, SIGKILL (filesystem guarantees)
- Concurrent safety: Prevent multiple containers mounting same workspace read-write

### Data Integrity
- Atomic writes: Agent uses atomic file operations (write temp, rename)
- Checkpoint files: JSON with schema version for forward compatibility
- Corruption detection: Agent validates checkpoint integrity on resume

## Technology and Data Variations

**Mount Types**:
- Bind mount: Host directory directly mounted (./workspace:/workspace)
- Named volume: Docker-managed volume (workspace-1:/workspace)
- tmpfs: In-memory workspace (no persistence, maximum security)

**Filesystem Types**:
- ext4: Standard Linux filesystem, good performance
- XFS: Better for large files, supports quotas
- Btrfs: Snapshots for rollback, copy-on-write
- NFS: Network-mounted workspace (shared across hosts)

**Workspace Content**:
- Git repositories: Code, commits, branches
- Build artifacts: Compiled binaries, object files
- Logs: Agent execution logs, debug traces
- Checkpoints: Progress tracking, resumable state

## Open Issues

**OI-001**: QEMU VM workspace persistence not tested (separate disk image vs host mount)
**OI-002**: Workspace backup automation not implemented (future: S3 sync)
**OI-003**: Workspace snapshot/restore for rollback not available
**OI-004**: Cross-host workspace migration not supported (single-host only)
**OI-005**: Concurrent read-only access by multiple agents unclear (safe or not)

## Frequency of Occurrence

- **Expected**: 100% of agent launches use persistent workspace (standard practice)
- **Resume Scenarios**: 20-30% of agent runs resume from prior checkpoint
- **Crash Recovery**: 5-10 workspace recoveries per week (OOM kills, crashes)

## Assumptions

**A-001**: Agent respects /workspace convention (writes outputs there)
**A-002**: Host filesystem reliable (SSD, no impending failure)
**A-003**: Developer uses consistent mount paths across restarts
**A-004**: Agent implements checkpoint/resume logic (not automatic)
**A-005**: Workspace data size reasonable (<100GB typical, <500GB maximum)

## Acceptance Criteria

- [ ] Agent writes file to /workspace, visible on host filesystem immediately
- [ ] Container restart preserves all workspace data (no data loss)
- [ ] Container crash (OOM kill) preserves workspace data
- [ ] Agent resumes from checkpoint after restart
- [ ] Named Docker volume persists across container deletion
- [ ] Bind mount permissions allow agent (UID 1000) to read/write
- [ ] Multiple agents cannot mount same workspace read-write simultaneously
- [ ] Workspace survives host reboot (if using named volume or bind mount)
- [ ] Developer can backup workspace via standard filesystem tools (rsync, tar)
- [ ] Workspace data accessible from host without container running

## Notes

- This is essential for long-running agent tasks (hours/days)
- Checkpoint/resume logic must be implemented by agent code (not automatic)
- Consider workspace snapshots for rollback (Btrfs, LVM snapshots)
- Backup automation desirable (rsync to S3, scheduled snapshots)
- QEMU VM workspace requires separate disk image (qcow2) vs container bind mount
- Concurrent read-only access may be safe (needs testing with git repos)

## Related Use Cases

- **UC-001**: Launch Autonomous Coding Agent (workspace stores agent outputs)
- **UC-002**: Git Repository Operations via Proxy (cloned repos persist in workspace)
- **UC-004**: Resource-Limited Sandbox Execution (disk quota affects workspace size)

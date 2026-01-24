# ADR-003: Seccomp Allow-List Design

## Status

Accepted (implemented in `configs/seccomp-profile.json`)

## Date

2026-01-05

## Context

Containerized agents execute arbitrary code including development workflows (git, npm, python, gcc). The Linux kernel exposes 300+ system calls, many of which can be used for privilege escalation, container escape, or host system manipulation.

### Threat Model

| Attack Vector | Syscalls Involved | Risk Level |
|--------------|-------------------|------------|
| Kernel module loading | `init_module`, `finit_module`, `delete_module` | Critical |
| System manipulation | `reboot`, `swapon`, `swapoff`, `kexec_load` | Critical |
| Mount namespace escape | `mount`, `umount`, `pivot_root` | High |
| Raw disk/device access | `iopl`, `ioperm`, `mknod` (block devices) | High |
| Ptrace-based escape | `ptrace` (unrestricted) | High |
| Time manipulation | `settimeofday`, `adjtimex` (unrestricted) | Medium |
| Namespace manipulation | `setns`, `unshare` (unrestricted) | Medium |

### Docker Default Seccomp Profile

Docker's default profile:
- Blocks approximately 44 syscalls
- Allows approximately 300+ syscalls
- Designed for broad compatibility, not security hardening

For a security-focused sandbox handling sensitive credentials and production data, the default profile is insufficient.

## Decision

Implement a conservative allow-list seccomp profile with explicit syscall permissions and default deny.

### Design Principles

1. **Default deny**: `SCMP_ACT_ERRNO` for any syscall not explicitly allowed
2. **Explicit allow-list**: 200+ syscalls permitted for development workflow compatibility
3. **Block dangerous syscalls**: No kernel modules, system reboot, raw I/O
4. **Architecture-specific**: Target x86_64 with x86 and x32 subarchitectures

### Profile Structure

```json
{
  "defaultAction": "SCMP_ACT_ERRNO",
  "defaultErrnoRet": 1,
  "archMap": [
    {
      "architecture": "SCMP_ARCH_X86_64",
      "subArchitectures": ["SCMP_ARCH_X86", "SCMP_ARCH_X32"]
    }
  ],
  "syscalls": [
    {
      "names": ["read", "write", "open", "...200+ syscalls..."],
      "action": "SCMP_ACT_ALLOW"
    }
  ]
}
```

### Syscall Categories

#### Allowed Categories (Development Workflow Support)

| Category | Example Syscalls | Rationale |
|----------|-----------------|-----------|
| File I/O | `read`, `write`, `open`, `openat`, `close`, `stat`, `fstat` | Essential for all operations |
| Directory | `mkdir`, `rmdir`, `readdir`, `chdir`, `getcwd` | Workspace management |
| Process | `fork`, `vfork`, `clone`, `execve`, `exit`, `wait4` | Running build tools, compilers |
| Memory | `mmap`, `munmap`, `mprotect`, `brk`, `mremap` | Application memory management |
| Network | `socket`, `connect`, `bind`, `listen`, `accept`, `send`, `recv` | Git, npm, pip downloads |
| Signals | `rt_sigaction`, `rt_sigprocmask`, `kill`, `tgkill` | Process control |
| Time | `clock_gettime`, `gettimeofday`, `nanosleep` | Timing, sleeps |
| IPC | `pipe`, `socketpair`, `eventfd`, `epoll_*` | Inter-process communication |
| Sync | `futex`, `flock`, `fcntl` | Lock management |
| User/Group | `getuid`, `getgid`, `setuid`, `setgid`, `setgroups` | Privilege dropping |
| Extended Attrs | `getxattr`, `setxattr`, `listxattr` | Filesystem metadata |
| Async I/O | `io_uring_*`, `io_submit`, `io_getevents` | Modern async I/O |
| Landlock | `landlock_*` | Additional sandboxing |

#### Blocked Syscalls (Security Critical)

| Syscall | Risk | Blocked Reason |
|---------|------|----------------|
| `init_module` | Critical | Load kernel module (rootkit injection) |
| `finit_module` | Critical | Load kernel module from file descriptor |
| `delete_module` | Critical | Unload kernel module |
| `reboot` | Critical | System shutdown/restart |
| `kexec_load` | Critical | Load new kernel (bypass security) |
| `kexec_file_load` | Critical | Load new kernel from file |
| `mount` | High | Escape container filesystem namespace |
| `umount` | High | Unmount filesystems |
| `umount2` | High | Unmount with flags |
| `pivot_root` | High | Change root filesystem |
| `swapon` | High | Enable swap (resource exhaustion) |
| `swapoff` | High | Disable swap |
| `iopl` | High | Change I/O privilege level |
| `ioperm` | High | Set I/O port permissions |
| `syslog` | Medium | Read kernel ring buffer |
| `acct` | Medium | Enable process accounting |
| `quotactl` | Medium | Manipulate disk quotas |
| `sysfs` | Low | Filesystem type information |
| `uselib` | Low | Legacy library loading |
| `nfsservctl` | Low | NFS server control (removed in 3.1) |

#### Conditional Syscalls (Argument Filtering)

```json
{
  "names": ["clone", "clone3"],
  "action": "SCMP_ACT_ALLOW",
  "args": [],
  "comment": "Allow clone for process creation"
}
```

```json
{
  "names": ["personality"],
  "action": "SCMP_ACT_ALLOW",
  "args": [
    {"index": 0, "value": 0, "op": "SCMP_CMP_EQ"},
    {"index": 0, "value": 8, "op": "SCMP_CMP_EQ"},
    {"index": 0, "value": 131072, "op": "SCMP_CMP_EQ"},
    {"index": 0, "value": 131080, "op": "SCMP_CMP_EQ"},
    {"index": 0, "value": 4294967295, "op": "SCMP_CMP_EQ"}
  ]
}
```

The `personality` syscall is restricted to safe values:
- `0`: Linux default
- `8`: ADDR_NO_RANDOMIZE (disable ASLR for debugging)
- `131072`: UNAME26 (report kernel as 2.6.x)
- `131080`: Combination of above
- `4294967295`: Query current personality

### Integration

Applied in Docker via security option:

```yaml
# runtimes/docker/docker-compose.yml
security_opt:
  - seccomp:../../configs/seccomp-profile.json
```

Applied in launch script:

```bash
# scripts/sandbox-launch.sh
docker run --security-opt seccomp=$PROJECT_ROOT/configs/seccomp-profile.json ...
```

## Consequences

### Positive

- **Reduced attack surface**: 100+ dangerous syscalls blocked by default
- **Defense-in-depth**: Even if capabilities or namespace isolation fails, syscall filtering limits exploit potential
- **Development compatibility**: 200+ allowed syscalls support git, npm, python, gcc, rust toolchains
- **Explicit documentation**: Allow-list makes permitted operations clear for security audits
- **Kernel protection**: No kernel module loading prevents rootkit installation

### Negative

- **May block legitimate operations**: Some unusual syscalls may be needed and blocked
- **Maintenance burden**: New syscalls in kernel updates may need addition
- **Testing required**: Must verify development workflows function correctly
- **Performance overhead**: Minimal (~1% for syscall-heavy workloads) but non-zero
- **Architecture-specific**: x86_64 only; ARM support requires separate profile

### Mitigations

- **Comprehensive testing**: Test git clone, npm install, python packages, rust build, gcc compilation
- **Syscall tracing**: Use `strace` to identify blocked calls when agents fail
- **Iterative refinement**: Add syscalls as needed based on actual agent failures
- **Logging**: Enable seccomp audit logging to capture blocked calls
- **Fallback procedure**: Document how to add syscalls if blocking causes issues

## Testing Plan

### Positive Tests (Should Work)

| Operation | Commands | Expected Result |
|-----------|----------|-----------------|
| Git operations | `git clone`, `git commit`, `git push` | Success |
| Node.js | `npm install`, `npm run build` | Success |
| Python | `pip install`, `python script.py` | Success |
| Compilation | `gcc`, `g++`, `rustc`, `cargo build` | Success |
| File operations | `cp`, `mv`, `rm`, `mkdir`, `chmod` | Success |
| Process management | `ps`, `top`, `kill`, background jobs | Success |
| Networking | `curl`, `wget`, internal socket communication | Success |

### Negative Tests (Should Fail)

| Operation | Command | Expected Result |
|-----------|---------|-----------------|
| Kernel module | `insmod`, `modprobe` | EPERM (syscall blocked) |
| Reboot | `reboot`, `shutdown` | EPERM |
| Mount | `mount /dev/sda1 /mnt` | EPERM |
| Raw disk | Create block device node | EPERM |
| Ptrace escape | Ptrace attach to host process | EPERM |

### Escape Attempt Tests

| Exploit | CVE Reference | Expected Result |
|---------|---------------|-----------------|
| Dirty Pipe | CVE-2022-0847 | Blocked (splice restrictions) |
| runC breakout | CVE-2019-5736 | Blocked (no /proc/self/exe overwrite) |
| OverlayFS escape | CVE-2021-3493 | Blocked (no unshare/setns) |

## Alternatives Considered

### Alternative A: Docker Default Seccomp

**Rejected because**:
- Allows 300+ syscalls including many dangerous ones
- Designed for compatibility, not security
- Insufficient for handling credentials and production data

### Alternative B: Deny-List Approach

Block specific dangerous syscalls, allow everything else.

**Rejected because**:
- New kernel syscalls automatically allowed (unsafe default)
- Harder to audit (must enumerate all blocked)
- Miss new attack vectors as kernel evolves

### Alternative C: gVisor/Kata Containers

Use userspace kernel or microVM for syscall interception.

**Rejected because**:
- Additional complexity and overhead
- QEMU already provides hardware isolation for highest-security cases
- Seccomp sufficient for Docker runtime security layer
- Can add gVisor later if seccomp proves insufficient

### Alternative D: AppArmor/SELinux Only

Rely on MAC (Mandatory Access Control) without seccomp.

**Rejected because**:
- MAC controls file/network access, not syscall filtering
- Seccomp provides orthogonal protection layer
- Defense-in-depth requires both (seccomp + capabilities + MAC)

## Related Documents

- Seccomp Profile: `configs/seccomp-profile.json` (implementation)
- Docker Compose: `runtimes/docker/docker-compose.yml` (integration)
- Launch Script: `scripts/sandbox-launch.sh` (runtime application)
- Project Intake: `.aiwg/intake/project-intake.md` (security requirements)

## References

- Linux seccomp documentation: https://www.kernel.org/doc/html/latest/userspace-api/seccomp_filter.html
- Docker seccomp profiles: https://docs.docker.com/engine/security/seccomp/
- Default Docker seccomp profile: https://github.com/moby/moby/blob/master/profiles/seccomp/default.json

## Revision History

| Date | Author | Change |
|------|--------|--------|
| 2026-01-05 | Architecture Team | Initial implementation with 200+ allowed syscalls |

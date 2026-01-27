# Seccomp Profile Documentation

**File:** `configs/seccomp-agent.json`
**Version:** 1.1
**Date:** 2026-01-24

## Overview

This seccomp (secure computing) profile restricts the system calls available to agent sandboxes. It uses a default-deny approach with an explicit allowlist of safe syscalls and documents each blocked syscall category.

## Profile Design

### Default Action

```json
"defaultAction": "SCMP_ACT_ERRNO"
```

Any syscall not explicitly allowed returns `EPERM` (Operation not permitted). This is safer than the kernel default which allows most syscalls.

### Supported Architectures

- `SCMP_ARCH_X86_64` - 64-bit x86 (primary)
- `SCMP_ARCH_X86` - 32-bit x86 (compatibility)
- `SCMP_ARCH_AARCH64` - 64-bit ARM
- `SCMP_ARCH_ARM` - 32-bit ARM

## Blocked Syscall Categories

### Container Escape Prevention

| Syscall | Risk | Description |
|---------|------|-------------|
| `ptrace` | Critical | Process tracing allows debugging other processes, reading memory, and can be used to escape containers |
| `setns` | Critical | Enter another namespace - primary container escape vector |
| `unshare` | Critical | Create new namespaces - can escape isolation |
| `personality` | High | Change execution domain - potential escape vector |

### Filesystem Manipulation

| Syscall | Risk | Description |
|---------|------|-------------|
| `mount` | Critical | Mount filesystems - could mount host filesystem |
| `umount` | Critical | Unmount filesystems - disrupt container |
| `umount2` | Critical | Unmount with flags |
| `pivot_root` | Critical | Change root filesystem |
| `chroot` | High | Change root directory |
| `open_by_handle_at` | High | Open file by handle - bypasses mount namespace |

### Kernel Manipulation

| Syscall | Risk | Description |
|---------|------|-------------|
| `bpf` | Critical | Load BPF programs - kernel-level code execution |
| `kexec_load` | Critical | Load new kernel - complete system compromise |
| `kexec_file_load` | Critical | Load kernel from file |
| `init_module` | Critical | Load kernel module - rootkit installation |
| `finit_module` | Critical | Load kernel module from file descriptor |
| `delete_module` | High | Unload kernel module |
| `create_module` | High | Create loadable module entry (deprecated) |
| `get_kernel_syms` | Medium | Get kernel symbol table (deprecated) |
| `query_module` | Medium | Query kernel module (deprecated) |

### System Control

| Syscall | Risk | Description |
|---------|------|-------------|
| `reboot` | High | Reboot system - denial of service |
| `syslog` | Medium | Read/control kernel log - information disclosure |
| `_sysctl` | Medium | Kernel parameter manipulation (deprecated) |

### Time Manipulation

| Syscall | Risk | Description |
|---------|------|-------------|
| `clock_settime` | High | Set system clock - affects logging, security |
| `clock_adjtime` | High | Adjust system clock |
| `settimeofday` | High | Set system time |
| `adjtimex` | High | Tune kernel clock |

### Hardware Access

| Syscall | Risk | Description |
|---------|------|-------------|
| `ioperm` | Critical | Set I/O port permissions - direct hardware access |
| `iopl` | Critical | Change I/O privilege level |

### Memory Policy

| Syscall | Risk | Description |
|---------|------|-------------|
| `get_mempolicy` | Low | Get NUMA memory policy |
| `set_mempolicy` | Medium | Set NUMA memory policy |
| `mbind` | Medium | Bind memory to NUMA node |
| `migrate_pages` | Medium | Move pages between NUMA nodes |
| `move_pages` | Medium | Move pages in process address space |

### Capability and Security

| Syscall | Risk | Description |
|---------|------|-------------|
| `capset` | High | Set process capabilities - privilege escalation |
| `seccomp` | High | Modify seccomp filter - weaken security |

### Kernel Keyring

| Syscall | Risk | Description |
|---------|------|-------------|
| `add_key` | Medium | Add key to kernel keyring |
| `keyctl` | Medium | Manipulate kernel keyring |
| `request_key` | Medium | Request key from kernel keyring |

### Miscellaneous Dangerous

| Syscall | Risk | Description |
|---------|------|-------------|
| `userfaultfd` | High | User-space page fault handling - exploitation primitive |
| `perf_event_open` | High | Performance monitoring - side-channel attacks |
| `acct` | Medium | Process accounting - host information disclosure |
| `kcmp` | Medium | Compare kernel resources - escape primitive |
| `lookup_dcookie` | Low | Directory entry cookie - profiling information |
| `nfsservctl` | Medium | NFS server control (deprecated) |
| `quotactl` | Medium | Disk quota control |
| `quotactl_fd` | Medium | Disk quota control via fd |
| `setdomainname` | Low | Set NIS domain name |
| `sethostname` | Low | Set hostname |
| `swapon` | High | Enable swap - affect host memory |
| `swapoff` | High | Disable swap |
| `sysfs` | Low | Sysfs operations (deprecated) |
| `uselib` | Medium | Load shared library (deprecated, exploitation target) |
| `vhangup` | Low | Virtual hangup - affect terminals |
| `vm86` | Low | Enter VM86 mode (x86 only, deprecated) |
| `vm86old` | Low | Old VM86 interface |

## Allowed Syscalls

The profile allows approximately 200 syscalls required for normal agent operation, including:

### Process Management
- `fork`, `vfork`, `clone`, `clone3` - Create processes
- `execve`, `execveat` - Execute programs
- `exit`, `exit_group` - Terminate processes
- `wait4`, `waitid`, `waitpid` - Wait for child processes
- `kill`, `tgkill`, `tkill` - Send signals

### Memory Management
- `mmap`, `munmap`, `mremap` - Memory mapping
- `mprotect` - Memory protection
- `brk` - Heap management
- `madvise` - Memory hints
- `mlock`, `munlock`, `mlockall`, `munlockall` - Memory locking

### File Operations
- `open`, `openat`, `openat2` - Open files
- `read`, `write`, `pread64`, `pwrite64` - I/O
- `close` - Close descriptors
- `stat`, `fstat`, `lstat`, `statx` - File status
- `chmod`, `fchmod`, `fchmodat` - Change permissions
- `chown`, `fchown`, `lchown` - Change ownership

### Network
- `socket`, `socketpair` - Create sockets
- `bind`, `listen`, `accept`, `accept4` - Server operations
- `connect` - Client connections
- `send`, `recv`, `sendto`, `recvfrom` - Data transfer
- `sendmsg`, `recvmsg`, `sendmmsg`, `recvmmsg` - Message operations
- `shutdown` - Shutdown connection

### Signals
- `rt_sigaction`, `rt_sigprocmask` - Signal handling
- `rt_sigreturn` - Return from signal handler
- `sigaltstack` - Alternate signal stack

### Time
- `clock_gettime`, `clock_getres` - Get time
- `gettimeofday`, `time` - Get time (legacy)
- `nanosleep`, `clock_nanosleep` - Sleep

### Misc
- `getrandom` - Random numbers
- `prctl` - Process control (with restrictions)
- `ioctl` - Device control (with restrictions)
- `sysinfo` - System information

## Testing the Profile

### Verify blocked syscalls

```bash
# Test ptrace block
docker run --rm \
  --security-opt seccomp=configs/seccomp-agent.json \
  alpine strace ls
# Expected: strace fails with "Operation not permitted"

# Test mount block
docker run --rm \
  --security-opt seccomp=configs/seccomp-agent.json \
  alpine mount -t tmpfs none /tmp
# Expected: mount fails with "Operation not permitted"

# Test setns block
docker run --rm \
  --security-opt seccomp=configs/seccomp-agent.json \
  alpine nsenter --target 1 --mount
# Expected: nsenter fails with "Operation not permitted"
```

### Verify allowed syscalls

```bash
# Normal operations should work
docker run --rm \
  --security-opt seccomp=configs/seccomp-agent.json \
  alpine sh -c 'echo "Hello"; ls /; cat /etc/os-release'
# Expected: All commands succeed

# Network operations should work
docker run --rm \
  --security-opt seccomp=configs/seccomp-agent.json \
  alpine wget -O- http://example.com
# Expected: Download succeeds (if network available)
```

## Monitoring Seccomp Violations

### Enable audit logging

```bash
# View seccomp audit logs (requires auditd)
ausearch -m SECCOMP

# Example output:
# type=SECCOMP msg=audit(1706123456.789:1234): auid=1000 uid=0 gid=0 ses=1
#   pid=5678 comm="strace" exe="/usr/bin/strace" sig=0 arch=c000003e
#   syscall=101 compat=0 ip=0x7f1234567890 code=0x50000
```

### Syscall number reference

```bash
# Look up syscall number
ausyscall 101
# Output: ptrace

# Full syscall table
ausyscall --dump
```

## Updating the Profile

### Adding a new allowed syscall

1. Identify the syscall needed (from logs or testing)
2. Research security implications
3. Add to the allowlist in `seccomp-agent.json`
4. Test that agent workload now succeeds
5. Document why it was added

### Removing an allowed syscall

1. Check if any agent workloads use it
2. Remove from allowlist
3. Test all agent workloads
4. Document the change

## Comparison with Docker Default

Docker's default seccomp profile is more permissive. Key differences:

| Syscall | Docker Default | Our Profile |
|---------|----------------|-------------|
| `ptrace` | Blocked | Blocked |
| `mount` | Blocked | Blocked |
| `setns` | Blocked | Blocked |
| `unshare` | Blocked | Blocked |
| `personality` | Allowed (restricted) | Blocked |
| `bpf` | Blocked | Blocked |
| `userfaultfd` | Allowed | Blocked |
| `capset` | Allowed | Blocked |
| `seccomp` | Allowed | Blocked |
| `clock_settime` | Allowed | Blocked |
| `add_key` | Allowed | Blocked |

Our profile is more restrictive to minimize attack surface for agent sandboxes.

## References

- [Docker Seccomp Documentation](https://docs.docker.com/engine/security/seccomp/)
- [Linux Syscall Table](https://filippo.io/linux-syscall-table/)
- [seccomp-bpf Documentation](https://www.kernel.org/doc/html/latest/userspace-api/seccomp_filter.html)
- [Docker Default Profile](https://github.com/moby/moby/blob/master/profiles/seccomp/default.json)

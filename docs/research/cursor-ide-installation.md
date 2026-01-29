# Cursor IDE Installation Research

**Research Date:** 2026-01-28
**Target Platform:** Ubuntu 24.04 (headless server, AMD64)
**Target User:** agent
**Use Case:** Automated VM provisioning via cloud-init

## Executive Summary

**Technology:** Cursor IDE v2.4.22
**Purpose:** AI-powered code editor based on VSCode/Electron
**Recommendation:** Use official .deb package for automated headless installation
**Installation Method:** Non-interactive APT installation via cloud-init
**Confidence:** High

Cursor IDE provides official .deb packages suitable for automated Ubuntu installation. The package has extensive GUI dependencies but can be installed headlessly. The editor uses project-level `.cursorrules` files for AI behavior configuration and stores user settings in `~/.config/Cursor/`.

## Overview

- **Repository:** https://github.com/getcursor/cursor
- **Official Site:** https://cursor.com
- **License:** Proprietary (free tier available)
- **Architecture:** Electron-based (VSCode fork)
- **Latest Version:** 2.4.22 (as of 2026-01-26)
- **Package Size:** ~168 MB (deb), ~274 MB (AppImage)
- **Installed Size:** ~740 MB
- **Maintainer:** Cursor team <hi@cursor.com>

## Download URLs

### Official Download Endpoints

Cursor uses a dynamic download service that always provides the latest version:

#### AMD64 (x86_64)

**Debian Package (Recommended for Ubuntu):**
```
https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4
```
- Resolves to: `https://downloads.cursor.com/production/.../cursor_2.4.22_amd64.deb`
- Size: ~176 MB
- Content-Type: application/x-debian-package

**AppImage (Portable):**
```
https://api2.cursor.sh/updates/download/golden/linux-x64/cursor/2.4
```
- Resolves to: `https://downloads.cursor.com/production/.../Cursor-2.4.22-x86_64.AppImage`
- Size: ~287 MB
- Content-Type: application/octet-stream
- Requires FUSE to run

**RPM Package:**
```
https://api2.cursor.sh/updates/download/golden/linux-x64-rpm/cursor/2.4
```

#### ARM64 (aarch64)

Similar URLs available with `linux-arm64` instead of `linux-x64`:
- `linux-arm64-deb` - Debian package
- `linux-arm64` - AppImage
- `linux-arm64-rpm` - RPM package

## Dependencies

### Required System Dependencies

From the .deb package control file:

**Essential:**
- ca-certificates
- libasound2 (>= 1.0.17)
- libatk-bridge2.0-0 (>= 2.5.3)
- libatk1.0-0 (>= 2.11.90)
- libatspi2.0-0 (>= 2.9.90)
- libc6 (>= 2.28)
- libcairo2 (>= 1.6.0)
- libcups2 (>= 1.6.0)
- libcurl3-gnutls | libcurl3-nss | libcurl4 | libcurl3
- libdbus-1-3 (>= 1.9.14)
- libexpat1 (>= 2.1~beta3)
- libgbm1 (>= 17.1.0~rc2)
- libglib2.0-0 (>= 2.39.4)
- libgtk-3-0 (>= 3.9.10) | libgtk-4-1
- libnspr4 (>= 2:4.9-2~)
- libnss3 (>= 3.26)
- libpango-1.0-0 (>= 1.14.0)
- libstdc++6 (>= 4.8)
- libudev1 (>= 183)
- libx11-6 (>= 2:1.4.99.1)
- libxcb1 (>= 1.9.2)
- libxcomposite1 (>= 1:0.4.4-1)
- libxdamage1 (>= 1:1.1)
- libxext6
- libxfixes3
- libxkbcommon0 (>= 0.5.0)
- libxkbfile1 (>= 1:1.1.0)
- libxrandr2
- xdg-utils (>= 1.0.2)

**Recommended:**
- libvulkan1 (for GPU acceleration)

### Ubuntu 24.04 Compatibility

All dependencies are available in Ubuntu 24.04 repositories. The package manager will automatically resolve and install dependencies.

## Installation Methods

### Method 1: Debian Package (Recommended)

**For cloud-init / automated provisioning:**

```bash
# Download latest version
wget -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4

# Install with APT (handles dependencies automatically)
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb

# Clean up
rm /tmp/cursor.deb
```

**Alternative: Direct install with dpkg + apt-get fix:**

```bash
wget -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4
dpkg -i /tmp/cursor.deb || true
apt-get install -y -f
rm /tmp/cursor.deb
```

**One-liner for testing:**

```bash
wget -qO- https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4 | dpkg -i - || apt-get install -y -f
```

### Method 2: AppImage (Alternative)

AppImage is portable but requires FUSE and manual setup:

```bash
# Download AppImage
wget -O /usr/local/bin/cursor https://api2.cursor.sh/updates/download/golden/linux-x64/cursor/2.4

# Make executable
chmod +x /usr/local/bin/cursor

# Requires FUSE2 for AppImage runtime
apt-get install -y libfuse2
```

**Note:** AppImage is less suitable for automated provisioning as it may require additional desktop integration setup.

## Installation Paths

After .deb installation:

**Binaries:**
- `/usr/share/cursor/bin/cursor` - Main executable
- `/usr/share/cursor/bin/cursor-tunnel` - Remote tunnel binary
- `/usr/share/cursor/bin/code-tunnel` - VSCode-compatible tunnel
- Symlink created by postinst script (likely `/usr/bin/cursor`)

**Application Files:**
- `/usr/share/cursor/` - Main installation directory
- `/usr/share/applications/cursor.desktop` - Desktop launcher
- `/usr/share/applications/cursor-url-handler.desktop` - URL handler
- `/usr/share/bash-completion/completions/cursor` - Bash completions

**Package Info:**
- Package: cursor
- Section: devel
- Architecture: amd64

## Configuration

### User Configuration Directory

User-specific settings stored in:
```
~/.config/Cursor/
```

Similar to VSCode structure:
- `~/.config/Cursor/User/settings.json` - User settings
- `~/.config/Cursor/User/keybindings.json` - Keyboard shortcuts
- `~/.config/Cursor/extensions/` - Installed extensions
- `~/.config/Cursor/logs/` - Log files

### Project Configuration (.cursorrules)

**File:** `.cursorrules` (placed in project root directory)

**Purpose:** Project-specific AI behavior instructions

**Format:** Plain text file with custom rules/prompts for Cursor's AI

**Example:**
```
# Project: agentic-sandbox
# Language: Rust, Go, Python, Bash

## Coding Standards
- Use conventional commits (type(scope): subject)
- No AI attribution in commits
- Prefer explicit error handling over panics (Rust)
- Use absolute paths in scripts

## Architecture
- QEMU VMs for agent isolation
- gRPC for management plane communication
- WebSocket for real-time terminal output
- Cloud-init for VM provisioning

## Dependencies
- Rust: cargo workspace for multi-crate projects
- Go: use standard library where possible
- Python: pytest for testing

## Security
- No secrets in code or configs
- Principle of least privilege
- Audit logging for all agent actions
```

**Resources:**
- Awesome Cursor Rules: https://github.com/PatrickJS/awesome-cursorrules
- Community examples for various frameworks and languages

### CLI Usage

Cursor provides a command-line interface similar to VSCode:

```bash
# Open directory
cursor /path/to/project

# Open file
cursor /path/to/file.rs

# Install extension
cursor --install-extension <extension-id>

# List extensions
cursor --list-extensions

# Show version
cursor --version
```

## Headless Installation Notes

### No Display Required for Installation

The .deb package can be installed without X11/Wayland:
- Installation scripts do not require a display
- `DEBIAN_FRONTEND=noninteractive` prevents interactive prompts
- Dependencies are purely libraries, no display server needed for package installation

### Running Cursor Headlessly

**Limitations:**
- Cursor is a GUI application (Electron-based)
- Cannot run the editor UI without a display server
- Suitable for:
  - Pre-installing on VM images
  - Installing as part of agent environment preparation
  - Using CLI tools (cursor-tunnel for remote access)

**Workarounds for remote access:**
- X11 forwarding: `ssh -X user@host cursor`
- VNC server: Run a virtual display
- Cursor Tunnel: Built-in remote access feature
- VS Code Server: Cursor includes tunnel binaries for remote development

### Cloud-Init Integration

**Example cloud-init runcmd:**

```yaml
#cloud-config

packages:
  - wget
  - ca-certificates

runcmd:
  # Install Cursor IDE
  - wget -q -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4
  - DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb
  - rm /tmp/cursor.deb

  # Create .cursorrules for agent user
  - |
    cat > /home/agent/.cursorrules << 'EOF'
    # AI Agent Development Environment
    - Focus on Rust, Go, Python, and shell scripting
    - Follow project conventions in /home/agent/workspace
    - Prioritize security and least privilege principles
    EOF

  - chown agent:agent /home/agent/.cursorrules
  - chmod 644 /home/agent/.cursorrules
```

**Alternative with write_files:**

```yaml
#cloud-config

packages:
  - wget
  - ca-certificates

write_files:
  - path: /home/agent/.cursorrules
    owner: agent:agent
    permissions: '0644'
    content: |
      # AI Agent Development Environment
      - Focus on Rust, Go, Python, and shell scripting
      - Follow project conventions in /home/agent/workspace
      - Prioritize security and least privilege principles

runcmd:
  - wget -q -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4
  - DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb
  - rm /tmp/cursor.deb
```

## Verification Commands

After installation:

```bash
# Check if cursor is installed
dpkg -l | grep cursor

# Verify binary is accessible
which cursor
cursor --version

# Check installation directory
ls -lh /usr/share/cursor/

# Verify user can run (will fail without display, but tests binary)
su - agent -c "cursor --help"

# Check package info
dpkg -s cursor
```

## Strengths

1. Official .deb package with proper dependency management
2. Automatic updates through built-in updater
3. Large library of pre-configured .cursorrules for various frameworks
4. VSCode-compatible (easy migration for existing VSCode users)
5. Built-in AI features without additional plugins
6. Active development and community
7. Includes tunnel binaries for remote development

## Weaknesses

1. Proprietary software (not open source)
2. Large installation footprint (~740 MB installed)
3. Requires extensive GUI libraries even for headless install
4. No official apt repository (must download directly)
5. Electron-based (higher resource usage than native editors)
6. Cannot run editor UI without display server
7. AI features may require internet connectivity

## Security Considerations

**Installation Security:**
- Downloads from `downloads.cursor.com` (CDN-backed)
- No GPG signature verification available
- Installs setuid binary: `/usr/share/cursor/chrome-sandbox`
- Package maintainer scripts run as root during installation

**Runtime Security:**
- Electron sandbox enabled (chrome-sandbox)
- AI features send code to Cursor's servers for processing
- May store API keys and authentication tokens locally
- Network access required for AI functionality

**Mitigations:**
- Verify download URL is from `*.cursor.com` or `*.cursor.sh`
- Review package contents before installation: `dpkg-deb -c cursor.deb`
- Consider network isolation if used in sensitive environments
- Review privacy policy regarding code sent to AI services
- Use `.cursorrules` to avoid sending sensitive code patterns

## Alternative Solutions

### VS Code with AI Extensions

**Pros:**
- Open source (MIT licensed)
- Official apt repository available
- Smaller footprint without AI bundled
- Wide extension ecosystem
- Better security audit trail

**Cons:**
- Requires separate AI extension installation
- May need multiple extensions for Cursor-like features
- Configuration more complex

### JetBrains IDEs with AI Assistant

**Pros:**
- Professional IDEs with deep language support
- Built-in AI features available
- Strong refactoring tools

**Cons:**
- Proprietary and expensive (requires license)
- Much larger resource requirements
- Less suitable for automated provisioning

### Neovim/Vim with AI Plugins

**Pros:**
- Minimal resource usage
- Runs well in terminal/headless
- Highly customizable
- Fast startup time

**Cons:**
- Steep learning curve
- AI plugins less mature than Cursor
- Requires significant configuration

## Recommendations

### For Automated VM Provisioning

**Use the .deb package installation method:**
```bash
wget -q -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4
DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb
rm /tmp/cursor.deb
```

**Create project-specific .cursorrules:**
- Place in user home directory: `/home/agent/.cursorrules`
- Use for global agent development guidelines
- Override per-project in workspace directories

**For remote access:**
- Set up X11 forwarding for SSH sessions
- Or configure VNC server for GUI access
- Or use cursor-tunnel for web-based access

### Cloud-Init Best Practices

1. Use `DEBIAN_FRONTEND=noninteractive` to prevent prompts
2. Download to /tmp and clean up after installation
3. Set proper ownership for agent user files
4. Consider caching the .deb file if provisioning multiple VMs
5. Add verification step to confirm successful installation

### Configuration Strategy

**Global .cursorrules** (`/home/agent/.cursorrules`):
- General coding standards
- Security principles
- Common patterns for agent development

**Project .cursorrules** (`/home/agent/workspace/.cursorrules`):
- Project-specific architecture
- Technology stack details
- Repository conventions

## Example Implementation

**File:** `/home/roctinam/dev/agentic-sandbox/images/qemu/provision-vm.sh`

```bash
#!/bin/bash
# Cursor IDE installation for agent VM

set -euo pipefail

echo "Installing Cursor IDE..."

# Install dependencies first (should already be present)
apt-get update
apt-get install -y wget ca-certificates

# Download and install Cursor
CURSOR_DEB_URL="https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4"
wget -q -O /tmp/cursor.deb "$CURSOR_DEB_URL"

# Install (DEBIAN_FRONTEND=noninteractive prevents prompts)
DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb

# Verify installation
if ! command -v cursor &> /dev/null; then
    echo "ERROR: Cursor installation failed - binary not found"
    exit 1
fi

# Cleanup
rm /tmp/cursor.deb

echo "Cursor IDE installed successfully"
cursor --version

# Create .cursorrules for agent user
if [ -n "${AGENT_USER:-}" ]; then
    cat > "/home/${AGENT_USER}/.cursorrules" << 'EOF'
# Agentic Sandbox Development Environment
# AI coding assistant rules for persistent agent workspaces

## Architecture
- QEMU/KVM virtual machines for agent isolation
- gRPC for management plane (Rust)
- WebSocket for real-time terminal streams
- Cloud-init for automated provisioning

## Languages
- Rust: Primary language for agent runtime and management
- Go: Gateway and auxiliary services
- Python: Testing and automation scripts
- Bash: System provisioning and utilities

## Coding Standards
- Conventional commits: type(scope): subject
- No AI attribution in commits
- Imperative mood ("add feature" not "added feature")
- Absolute paths in scripts (cwd not persistent)
- Comprehensive error handling

## Security
- Principle of least privilege
- No secrets in code or configuration
- Audit logging for all agent actions
- Input validation and sanitization
- Resource limits enforcement

## Rust Conventions
- Cargo workspace for multi-crate projects
- Explicit error handling (Result<T, E>)
- Avoid panics in production code
- Use tokio for async runtime
- Prefer owned types over references in public APIs

## Testing
- Unit tests in same file as code
- Integration tests in tests/ directory
- Use pytest for Python tests
- Mock external dependencies
EOF

    chown "${AGENT_USER}:${AGENT_USER}" "/home/${AGENT_USER}/.cursorrules"
    chmod 644 "/home/${AGENT_USER}/.cursorrules"

    echo "Created .cursorrules for ${AGENT_USER}"
fi
```

**File:** `/home/roctinam/dev/agentic-sandbox/images/qemu/cloud-init/user-data.yaml`

```yaml
#cloud-config
# Cloud-init configuration for agent VM

users:
  - name: agent
    groups: sudo
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    lock_passwd: true
    ssh_authorized_keys:
      - ssh-ed25519 AAAAC3... user@host

packages:
  - build-essential
  - git
  - curl
  - wget
  - ca-certificates
  - rust-all
  - python3
  - python3-pip

write_files:
  - path: /home/agent/.cursorrules
    owner: agent:agent
    permissions: '0644'
    content: |
      # Agentic Sandbox Development Environment
      ## Architecture
      - QEMU/KVM for agent isolation
      - gRPC management plane (Rust)
      - WebSocket for terminal streams

      ## Languages
      - Rust, Go, Python, Bash

      ## Standards
      - Conventional commits
      - Absolute paths in scripts
      - Comprehensive error handling
      - Security first mindset

runcmd:
  # Install Cursor IDE
  - wget -q -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4
  - DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb
  - rm /tmp/cursor.deb

  # Verify installation
  - cursor --version || echo "WARNING: Cursor installation verification failed"

  # Setup workspace
  - mkdir -p /home/agent/workspace
  - chown -R agent:agent /home/agent/workspace

  # Install Rust components
  - su - agent -c "rustup component add rust-src rust-analyzer"

final_message: "Agent VM provisioned successfully with Cursor IDE"
```

## Testing Verification

```bash
# Test installation in clean Ubuntu 24.04 container
docker run -it --rm ubuntu:24.04 bash -c "
  apt-get update &&
  apt-get install -y wget ca-certificates &&
  wget -q -O /tmp/cursor.deb https://api2.cursor.sh/updates/download/golden/linux-x64-deb/cursor/2.4 &&
  DEBIAN_FRONTEND=noninteractive apt-get install -y /tmp/cursor.deb &&
  cursor --version &&
  echo 'Installation successful'
"
```

## References

- Official website: https://cursor.com
- Documentation: https://cursor.com/docs
- GitHub repository: https://github.com/getcursor/cursor
- Community forum: https://forum.cursor.com
- Awesome Cursor Rules: https://github.com/PatrickJS/awesome-cursorrules
- Download page: https://cursor.com/downloads

## Conclusion

Cursor IDE can be successfully installed on Ubuntu 24.04 headless servers using the official .deb package. The installation is suitable for automated provisioning via cloud-init or shell scripts. While the editor cannot run without a display server, the installation itself is non-interactive and robust.

For the agentic-sandbox project, Cursor provides a modern AI-assisted development environment that can be pre-installed on agent VMs. Combined with proper `.cursorrules` configuration, it offers an intelligent coding assistant tailored to the project's architecture and standards.

The recommended approach is to use the .deb package with `DEBIAN_FRONTEND=noninteractive` and create project-specific `.cursorrules` files during provisioning to ensure consistent AI behavior across all agent development environments.

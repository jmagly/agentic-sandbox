# Ubuntu 24.04 LTS Developer Environment Best Practices for Headless/Agentic AI Use

**Research Date:** 2026-01-28
**Target Platform:** Ubuntu 24.04 LTS (Noble Numbat)
**Focus:** Non-interactive, headless, autonomous agent environments

---

## Executive Summary

This research evaluates modern developer tooling for Ubuntu 24.04 LTS optimized for headless/non-interactive agentic AI use. Key findings:

**Critical Pre-installations:**
- Python: `uv` (all-in-one) > `pyenv` + `poetry` for multi-version management
- Rust: `rustup` (official installer)
- Build tools: `build-essential`, `cmake`, `ninja-build`
- Modern CLI: `ripgrep`, `fd-find`, `bat`, `jq`, `gh`
- Container runtime: Docker (apt repository method)

**On-Demand/Conditional:**
- TUI tools (lazygit, bottom, delta) - interactive only, not for agents
- Visual enhancements (starship, zoxide) - interactive shells only
- Language-specific tools via mise/asdf pattern

**Key Pattern:** Adopt a "guidance facility" using declarative tool manifests (devcontainer-style) for on-demand installation rather than bloating base images.

---

## 1. Python Development

### Multi-Version Management

**Winner: uv (Astral)**
- **Performance:** 10-100x faster than pip
- **Consolidation:** Replaces pip, pip-tools, poetry, pyenv, virtualenv, pipx, and twine
- **Installation:**
  ```bash
  curl -fsSL https://astral.sh/uv/install.sh | sh
  ```
- **Non-interactive:** Single static binary, no Python prerequisite
- **Use cases:** All Python workflows - projects, scripts, tools, version management

**Alternative: pyenv + poetry**
- **pyenv strengths:**
  - Per-project `.python-version` files
  - No root access required
  - Shell-script based (no dependencies)
  - Installation: `curl -fsSL https://pyenv.run | bash`
  - **Critical:** Install build dependencies first:
    ```bash
    sudo apt-get update
    sudo apt-get install -y build-essential libssl-dev zlib1g-dev \
      libbz2-dev libreadline-dev libsqlite3-dev curl git \
      libncursesw5-dev xz-utils tk-dev libxml2-dev libxmlsec1-dev \
      libffi-dev liblzma-dev
    ```

- **poetry strengths:**
  - Unified `pyproject.toml` configuration
  - Semantic versioning with dependency groups
  - Lock file reproducibility
  - Installation: `curl -fsSL https://install.python-poetry.org | python3 -`
  - **Consideration:** Slower than uv, overlapping functionality

**System Python (Ubuntu 24.04):**
- Version: Python 3.12.3
- **Use for:** System scripts only, not development
- **Never:** Install user packages with pip globally

### Dependency Management Comparison

| Tool | Speed | Features | Lock Files | Multi-Version | Package Publishing |
|------|-------|----------|------------|---------------|-------------------|
| **uv** | 10-100x | Universal | Yes | Yes | Yes |
| **poetry** | 1x | Projects | Yes | No | Yes |
| **pip-tools** | 1x | Basic | Yes | No | No |

### Python Tooling (Pre-install)

**Ruff - Linter/Formatter**
- **Why:** Replaces Flake8, Black, isort, pydocstyle, pyupgrade, autoflake
- **Performance:** 10-100x faster (0.4s for 250k LOC)
- **Installation:**
  ```bash
  # Via uv (recommended)
  uvx ruff

  # Via pip
  pip install ruff

  # Via apt (if available)
  sudo apt install ruff
  ```
- **Usage:** Single config file (`pyproject.toml` or `ruff.toml`)
- **Adoption:** Pandas, FastAPI, PyTorch, Anthropic SDK

**pipx - Isolated CLI Tools**
- **Purpose:** Install Python CLI tools in isolated virtual environments
- **Installation:**
  ```bash
  sudo apt install pipx
  pipx ensurepath
  ```
- **Use cases:**
  ```bash
  pipx install black
  pipx install pytest
  pipx run cowsay "isolated execution"
  ```
- **Advantages:**
  - No dependency conflicts
  - Clean uninstalls
  - No sudo for system-wide tools

**Recommendation for Agents:**
1. Use `uv` as default for all Python operations
2. Pre-install `ruff` for code quality
3. Use `pipx` for isolated CLI tools that aren't in uv

---

## 2. Rust Development

### Installation Pattern

**rustup (Official)**
- **Non-interactive installation:**
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source $HOME/.cargo/env
  ```
- **Environment variables:**
  - `RUSTUP_INIT_SKIP_PATH_CHECK=yes` - Skip PATH warnings
  - `CARGO_HOME=/opt/cargo` - Custom cargo location
  - `RUSTUP_HOME=/opt/rustup` - Custom rustup location

### Essential Toolchain Components

**Pre-install:**
```bash
rustup component add clippy rustfmt rust-analyzer
```

**Components:**
- `clippy` - Linting and static analysis
- `rustfmt` - Code formatting
- `rust-analyzer` - LSP for editor integration

**Optional (on-demand):**
```bash
# Cross-compilation targets
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-gnu

# Cargo extensions
cargo install cargo-watch    # Auto-rebuild on changes
cargo install cargo-audit     # Security vulnerability scanning
cargo install cargo-outdated  # Dependency update checking
```

### Cross-Compilation Support

**musl for static binaries:**
```bash
sudo apt install musl-tools
rustup target add x86_64-unknown-linux-musl
cargo build --target x86_64-unknown-linux-musl --release
```

---

## 3. Build Tools & Compilers

### Essential Packages

**build-essential**
```bash
sudo apt-get install -y build-essential
```
- Includes: `gcc`, `g++`, `make`, `libc6-dev`, `dpkg-dev`
- Version (24.04): GCC 13.2.0

### LLVM/Clang vs GCC

**When to use LLVM/Clang:**
- Better error messages and diagnostics
- Sanitizers (AddressSanitizer, UndefinedBehaviorSanitizer)
- WebAssembly compilation
- Modular architecture (MLIR, Flang)
- Cross-compilation flexibility

**When to use GCC:**
- Default Ubuntu toolchain
- Better optimization for x86_64 in some cases
- Wider platform support historically
- Established build scripts

**LLVM Installation (Ubuntu 24.04 Noble):**
```bash
# Add repository
wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | \
  sudo tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc

# Add source
echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble-22 main" | \
  sudo tee /etc/apt/sources.list.d/llvm.list

sudo apt-get update
sudo apt-get install -y clang-22 lldb-22 lld-22
```

**Quick install script:**
```bash
bash -c "$(wget -O - https://apt.llvm.org/llvm.sh)"
```

### Modern Build Systems

**CMake**
- **Ubuntu 24.04 version:** Available via apt (check `apt-cache policy cmake`)
- **Kitware PPA:** For latest versions
  ```bash
  wget -O - https://apt.kitware.com/keys/kitware-archive-latest.asc 2>/dev/null | \
    gpg --dearmor - | sudo tee /etc/apt/trusted.gpg.d/kitware.gpg >/dev/null
  sudo apt-add-repository 'deb https://apt.kitware.com/ubuntu/ noble main'
  sudo apt-get update
  sudo apt-get install -y cmake
  ```
- **From source:** Only if specific version required

**Meson**
- **Modern alternative** to CMake/autotools
- **Installation:**
  ```bash
  python3 -m pip install meson ninja
  # Or via apt
  sudo apt-get install -y meson ninja-build
  ```
- **Advantages:**
  - Faster builds than CMake
  - Python-based configuration (more readable)
  - Native Ninja integration
  - Better cross-compilation support
- **Adoption:** GNOME, systemd, Mesa, GStreamer

**Ninja**
- **Purpose:** Fast build executor (used by Meson/CMake)
- **Installation:**
  ```bash
  sudo apt-get install -y ninja-build
  ```
- **Performance:** Focus on speed over features
- **Usage:** Generated by Meson or CMake, not written manually

**Recommendation:**
- Pre-install: `build-essential`, `cmake`, `ninja-build`, `meson`
- On-demand: LLVM/Clang (project-specific)

---

## 4. Developer Productivity (CLI)

### Modern Replacements

**ripgrep - Fast grep alternative**
- **Installation:**
  ```bash
  sudo apt-get install -y ripgrep
  ```
- **Performance:** 32x faster than GNU grep (Linux kernel search)
- **Features:**
  - Respects `.gitignore` by default
  - Unicode support without slowdown
  - PCRE2 support (`-P` flag)
  - File-type filtering (`rg -tpy pattern`)
- **Usage:**
  ```bash
  rg "pattern"              # Search current directory
  rg -tpy "import" .        # Search Python files only
  rg --files-with-matches   # List matching files
  ```
- **Pre-install:** **Yes** - essential for code search

**fd - Modern find alternative**
- **Installation:**
  ```bash
  sudo apt-get install -y fd-find
  # Symlink if needed
  ln -s $(which fdfind) ~/.local/bin/fd
  ```
- **Performance:** 13-23x faster than find
- **Features:**
  - Intuitive syntax: `fd pattern`
  - Respects `.gitignore`
  - Smart case sensitivity
  - Parallel execution
- **Usage:**
  ```bash
  fd pattern               # Simple search
  fd -e py                 # Find Python files
  fd -e rs -x wc -l        # Execute on results
  ```
- **Pre-install:** **Yes** - significant productivity gain

**bat - cat with syntax highlighting**
- **Installation:**
  ```bash
  sudo apt-get install -y bat
  # On Ubuntu, executable is 'batcat'
  mkdir -p ~/.local/bin
  ln -s /usr/bin/batcat ~/.local/bin/bat
  ```
- **Features:**
  - Syntax highlighting (Pygments themes)
  - Git integration
  - Automatic paging
  - Non-printable character display
- **Usage:**
  ```bash
  bat README.md
  bat -A /etc/hosts        # Show non-printable chars
  bat -l json file.txt     # Force language
  ```
- **Integration:**
  ```bash
  # fzf preview
  fzf --preview "bat --color=always --style=numbers {}"

  # Man pager
  export MANPAGER="sh -c 'col -bx | bat -l man -p'"
  ```
- **Pre-install:** **Optional** - useful but not critical for agents

**eza - Modern ls replacement**
- **Installation:**
  ```bash
  sudo apt-get install -y eza
  ```
- **Features:**
  - Git status awareness
  - Color-coded file types
  - Tree view built-in
  - Extended attributes display
- **Pre-install:** **No** - nice to have, not essential

**delta - Git diff viewer**
- **Installation:**
  ```bash
  sudo apt-get install -y git-delta
  ```
- **Features:**
  - Syntax highlighting
  - Side-by-side view
  - Line numbers
  - Word-level diffs
- **Configuration:**
  ```bash
  git config --global core.pager delta
  git config --global interactive.diffFilter 'delta --color-only'
  git config --global delta.navigate true
  ```
- **Pre-install:** **No** - interactive tool, not useful for agents

### JSON/Data Processing

**jq - JSON processor**
- **Installation:**
  ```bash
  sudo apt-get install -y jq
  ```
- **Version (24.04):** 1.7.1
- **Usage:**
  ```bash
  curl api.example.com | jq '.data[]'
  cat package.json | jq '.version'
  jq -r '.[] | select(.type == "feature")' data.json
  ```
- **Pre-install:** **Yes** - critical for API/config workflows

**yq - YAML/XML processor**
- **Installation:**
  ```bash
  # Via snap
  sudo snap install yq

  # Via wget (static binary)
  wget https://github.com/mikefarah/yq/releases/latest/download/yq_linux_amd64 \
    -O ~/.local/bin/yq && chmod +x ~/.local/bin/yq
  ```
- **Pre-install:** **Conditional** - only if YAML-heavy workflows

### Shell Enhancements (Interactive Only - NOT for Agents)

**starship - Shell prompt**
- **Purpose:** Beautiful, informative prompts
- **Installation:**
  ```bash
  curl -sS https://starship.rs/install.sh | sh
  ```
- **Agent suitability:** **No** - interactive shells only

**direnv - Per-directory environments**
- **Purpose:** Auto-load environment variables
- **Installation:**
  ```bash
  sudo apt-get install -y direnv
  ```
- **Agent suitability:** **Yes** - useful for project isolation
- **Usage:**
  ```bash
  # In project directory
  echo "export DATABASE_URL=postgres://localhost/dev" > .envrc
  direnv allow
  ```
- **Pre-install:** **Yes** - valuable for multi-project agents

**zoxide - Smart cd**
- **Purpose:** Jump to frequently used directories
- **Installation:**
  ```bash
  curl -sSfL https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh | sh
  ```
- **Agent suitability:** **No** - interactive navigation only

### Git Tools

**gh - GitHub CLI**
- **Installation:**
  ```bash
  sudo apt-get install -y gh
  ```
- **Non-interactive usage:**
  ```bash
  # Set token
  export GH_TOKEN=ghp_xxx

  # Create PR
  gh pr create --title "feat: add feature" --body "description" --base main

  # List PRs
  gh pr list --state open --json number,title

  # View PR
  gh pr view 123 --json body,reviews
  ```
- **Pre-install:** **Yes** - essential for GitHub automation

**lazygit - TUI for git**
- **Agent suitability:** **No** - interactive TUI only

**Recommendation Summary:**
- **Pre-install:** ripgrep, fd-find, jq, gh, direnv
- **Optional:** bat (for debugging output)
- **Skip:** eza, delta, starship, zoxide, lazygit (interactive only)

---

## 5. Container & Virtualization

### Docker Installation (Ubuntu 24.04)

**Official apt repository method (RECOMMENDED):**
```bash
# Non-interactive setup
export DEBIAN_FRONTEND=noninteractive

# Install prerequisites
sudo apt-get update
sudo apt-get install -y ca-certificates curl

# Add Docker GPG key
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg \
  -o /etc/apt/keyrings/docker.asc
sudo chmod a+r /etc/apt/keyrings/docker.asc

# Add repository
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \
  https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# Install Docker
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io \
  docker-buildx-plugin docker-compose-plugin

# Post-install: non-root access
sudo usermod -aG docker $USER
```

**Convenience script (development only):**
```bash
# NOT recommended for production
curl -fsSL https://get.docker.com -o get-docker.sh
sudo sh get-docker.sh --dry-run  # Preview first
sudo sh get-docker.sh
```

**Why apt repository over convenience script:**
- Controlled version upgrades
- Better dependency management
- Suitable for production systems
- Allows version pinning

### Podman as Alternative

**Installation:**
```bash
sudo apt-get install -y podman
```

**Advantages over Docker:**
- **Daemonless:** No background service required
- **Rootless by default:** Enhanced security
- **Pod support:** Kubernetes-style pod management
- **Docker-compatible CLI:** Drop-in replacement

**When to prefer Podman:**
- Rootless container requirements
- Security-first environments
- Resource-constrained systems (no daemon overhead)
- Kubernetes pod development

**When to prefer Docker:**
- Ecosystem maturity (Docker Compose, Swarm)
- Broader CI/CD integration
- Team familiarity
- Commercial support options

### OCI Image Tools

**buildah - OCI image builder**
- **Installation:**
  ```bash
  sudo apt-get install -y buildah
  ```
- **Advantages:**
  - Scriptable builds (shell/Go)
  - No daemon required
  - Rootless builds
  - Dockerfile-free workflows
- **Use cases:**
  - Programmatic image creation
  - Integration into Go applications
  - Security-restricted environments

**skopeo - OCI image operations**
- **Installation:**
  ```bash
  sudo apt-get install -y skopeo
  ```
- **Use cases:**
  ```bash
  # Copy without Docker daemon
  skopeo copy docker://alpine:latest docker-archive:alpine.tar

  # Inspect remote images
  skopeo inspect docker://registry.fedoraproject.org/fedora:latest

  # Delete remote tags
  skopeo delete docker://localhost:5000/myimage:tag
  ```

**Recommendation:**
- **Base install:** Docker (apt repository method)
- **Security-focused:** Podman + buildah
- **Utility:** skopeo for registry operations

---

## 6. Database Clients (CLI)

### Available in Ubuntu 24.04

**PostgreSQL**
```bash
sudo apt-get install -y postgresql-client-16
```
- Package: `postgresql-client-16`
- Version: 16.11 (Noble updates)
- Tools: `psql`, `pg_dump`, `pg_restore`

**MySQL**
```bash
sudo apt-get install -y mysql-client
```
- Package: `mysql-client` (metapackage for 8.0)
- Version: 8.0.44
- Tools: `mysql`, `mysqldump`, `mysqlshow`

**Redis**
```bash
sudo apt-get install -y redis-tools
```
- Package: `redis-tools`
- Version: 7.0.15
- Tools: `redis-cli`, `redis-benchmark`

**SQLite**
```bash
sudo apt-get install -y sqlite3
```
- Package: `sqlite3`
- Version: 3.45.1
- Tools: `sqlite3` CLI

### Modern TUI Clients

**pgcli - PostgreSQL with autocomplete**
- **Installation:**
  ```bash
  sudo apt-get install -y pgcli
  # Or via pipx
  pipx install pgcli
  ```
- **Features:**
  - Context-aware autocomplete
  - Syntax highlighting
  - Multi-line editing
  - Better table formatting
- **Agent suitability:** **Limited** - autocomplete benefits interactive use

**mycli - MySQL with autocomplete**
- **Installation:**
  ```bash
  pipx install mycli
  ```
- **Similar features** to pgcli

**litecli - SQLite with autocomplete**
- **Installation:**
  ```bash
  pipx install litecli
  ```

**Recommendation:**
- **Pre-install:** Standard clients (postgresql-client, mysql-client, redis-tools, sqlite3)
- **On-demand:** Modern TUI clients (pgcli, mycli) - limited agent benefit
- **Consider:** Docker containers for full database servers (not just clients)

---

## 7. Network & API Tools

### HTTP Clients

**curl**
- **Installation:** Pre-installed on Ubuntu 24.04
- **Version:** 8.5.0
- **Usage:** Universal HTTP client
- **Pre-install:** **Yes** (already present)

**httpie**
- **Installation:**
  ```bash
  sudo apt-get install -y httpie
  ```
- **Features:**
  - Human-friendly syntax
  - JSON support
  - Session management
- **Pre-install:** **Optional**

**xh - Rust httpie alternative**
- **Installation:**
  ```bash
  sudo apt-get install -y xh  # Ubuntu 25.04+
  # Or via cargo
  cargo install xh
  ```
- **Advantages:**
  - Faster than httpie
  - Single static binary
  - HTTP/2 support
  - `--curl` flag for translation
- **Performance:** Improved startup speed
- **Pre-install:** **Optional** - install if HTTP/2 needed

**Recommendation:** curl + xh (xh for development, curl for production scripts)

### gRPC Tools

**grpcurl**
- **Installation:**
  ```bash
  # Via Go
  go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest

  # Or download binary from GitHub releases
  wget https://github.com/fullstorydev/grpcurl/releases/download/v1.8.9/grpcurl_1.8.9_linux_x86_64.tar.gz
  tar -xvf grpcurl_1.8.9_linux_x86_64.tar.gz -C ~/.local/bin
  ```
- **Use cases:**
  ```bash
  # List services
  grpcurl localhost:50051 list

  # Describe service
  grpcurl localhost:50051 describe myservice.MyService

  # Invoke RPC
  grpcurl -d '{"name": "test"}' localhost:50051 myservice.MyService/GetUser
  ```
- **Pre-install:** **Conditional** - only if gRPC services present

### WebSocket Tools

**websocat**
- **Installation:**
  ```bash
  cargo install websocat
  # Or download from GitHub releases
  ```
- **Use cases:**
  ```bash
  # Connect to WebSocket
  websocat ws://localhost:8080/ws

  # Server mode
  websocat -s 8080

  # Pipe TCP to WebSocket
  websocat tcp-l:127.0.0.1:5555 ws://echo.websocket.org/
  ```
- **Pre-install:** **Conditional** - only if WebSocket testing needed

**Recommendation:**
- **Always:** curl
- **gRPC projects:** grpcurl
- **WebSocket projects:** websocat
- **HTTP/2 development:** xh

---

## 8. Observability & Debugging

### System Monitoring

**htop - Interactive process viewer**
- **Installation:**
  ```bash
  sudo apt-get install -y htop
  ```
- **Pre-install:** **Optional** - interactive tool

**btop/bottom - Modern alternatives**
- **bottom installation:**
  ```bash
  # Download .deb
  curl -LO https://github.com/ClementTsang/bottom/releases/download/0.12.3/bottom_0.12.3-1_amd64.deb
  sudo dpkg -i bottom_0.12.3-1_amd64.deb
  ```
- **Features:**
  - Graphical widgets
  - CPU/Memory/Network/Disk
  - Process tree
  - Temperature sensors
- **Agent suitability:** **No** - visual interface for humans

**Recommendation:** Skip interactive monitors for agents, use programmatic tools:
```bash
# CPU usage
top -bn1 | grep "Cpu(s)"

# Memory
free -h

# Disk
df -h

# Process info
ps aux | grep process_name
```

### Debugging Tools

**strace - System call tracer**
- **Installation:**
  ```bash
  sudo apt-get install -y strace
  ```
- **Usage:**
  ```bash
  strace -c program         # Count syscalls
  strace -e open program    # Trace specific calls
  strace -p PID             # Attach to running process
  ```
- **Pre-install:** **Yes** - critical for debugging

**ltrace - Library call tracer**
- **Installation:**
  ```bash
  sudo apt-get install -y ltrace
  ```
- **Pre-install:** **Optional**

**perf - Performance analysis**
- **Installation:**
  ```bash
  sudo apt-get install -y linux-tools-common linux-tools-generic
  ```
- **Usage:**
  ```bash
  perf record ./program
  perf report
  ```
- **Pre-install:** **Optional** - specialized use case

**Recommendation:**
- **Pre-install:** strace
- **On-demand:** ltrace, perf

---

## 9. Non-Interactive Installation Patterns

### APT Best Practices

**Environment variables:**
```bash
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a  # Auto-restart services
```

**Full non-interactive apt install:**
```bash
#!/bin/bash
set -e

export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a

# Update package lists
apt-get update

# Install with no prompts
apt-get install -y \
  -o Dpkg::Options::="--force-confnew" \
  -o Dpkg::Options::="--force-confdef" \
  package1 package2 package3

# Clean up
apt-get clean
rm -rf /var/lib/apt/lists/*
```

**dpkg options:**
- `--force-confnew` - Use new config files
- `--force-confdef` - Use default config options
- `--force-confold` - Keep old config files

### Build from Source vs Packages

**Prefer packages when:**
- Tool is in Ubuntu repositories
- Version in repo is recent enough
- Security updates via apt
- Standard installation location

**Build from source when:**
- Specific version required
- Package not available
- Custom compilation flags needed
- Bleeding-edge features required

**Pattern for source builds:**
```bash
#!/bin/bash
set -e

# Install build dependencies
apt-get install -y build-essential cmake git

# Clone and build
git clone https://github.com/org/project.git /tmp/build
cd /tmp/build
git checkout v1.2.3

# Build
mkdir build && cd build
cmake -DCMAKE_BUILD_TYPE=Release ..
make -j$(nproc)
sudo make install

# Cleanup
cd / && rm -rf /tmp/build
```

### Tool Installation Scripts

**Rustup pattern:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
  sh -s -- -y --default-toolchain stable --profile minimal
```

**Node.js (via fnm):**
```bash
curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell
export PATH="$HOME/.local/share/fnm:$PATH"
eval "$(fnm env --use-on-cd)"
fnm install --lts
```

**Go:**
```bash
GO_VERSION=1.22.0
wget https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz
sudo rm -rf /usr/local/go
sudo tar -C /usr/local -xzf go${GO_VERSION}.linux-amd64.tar.gz
rm go${GO_VERSION}.linux-amd64.tar.gz
export PATH=$PATH:/usr/local/go/bin
```

---

## 10. Guidance Facility Pattern

### Tool Version Management Systems

**asdf - Multi-language version manager**
- **Installation:**
  ```bash
  git clone https://github.com/asdf-vm/asdf.git ~/.asdf --branch v0.14.0
  echo '. $HOME/.asdf/asdf.sh' >> ~/.bashrc
  ```
- **Plugin architecture:**
  ```bash
  asdf plugin add nodejs
  asdf plugin add python
  asdf plugin add ruby
  ```
- **Usage:**
  ```bash
  # .tool-versions file
  echo "nodejs 20.11.0" >> .tool-versions
  echo "python 3.12.1" >> .tool-versions
  asdf install
  ```
- **Strengths:**
  - Single `.tool-versions` file
  - Consistent CLI across languages
  - Supports legacy config files (.nvmrc, .ruby-version)

**mise (formerly rtx) - Modern asdf alternative**
- **Installation:**
  ```bash
  curl https://mise.run | sh
  ```
- **Advantages over asdf:**
  - Written in Rust (faster)
  - Built-in task runner (like make)
  - Environment variable management (like direnv)
  - Better error messages
  - Compatible with asdf plugins
- **Usage:**
  ```toml
  # mise.toml
  [tools]
  node = "20"
  python = "3.12"
  terraform = "1.7"

  [env]
  DATABASE_URL = "postgres://localhost/dev"

  [tasks.test]
  run = "pytest tests/"
  ```
- **On-demand execution:**
  ```bash
  mise exec node@20 -- node script.js
  ```
- **Pre-install:** **Recommended** for multi-language agents

**Devcontainer Pattern**
- **Declarative tool specification:**
  ```json
  {
    "name": "Agent Dev Environment",
    "image": "ubuntu:24.04",
    "features": {
      "ghcr.io/devcontainers/features/python:1": {
        "version": "3.12"
      },
      "ghcr.io/devcontainers/features/node:1": {
        "version": "20"
      },
      "ghcr.io/devcontainers/features/rust:1": {}
    },
    "customizations": {
      "vscode": {
        "extensions": ["ms-python.python"]
      }
    }
  }
  ```
- **Benefits:**
  - Version-controlled environment specs
  - Reproducible across machines
  - Composable feature modules
  - Tool validation before provisioning

### Recommended Guidance Facility for Agents

**Hybrid approach:**

1. **Base image** with critical tools:
   - System packages (build-essential, git, curl, jq)
   - Core runtimes (Python system version, uv)
   - Essential CLI tools (ripgrep, fd, gh)

2. **mise for language versions:**
   - Project-specific tool versions via `mise.toml`
   - Automatic environment activation on directory change
   - On-demand tool installation

3. **Devcontainer-style manifest** for capabilities:
   ```yaml
   # agent-manifest.yml
   runtime_requirements:
     python:
       versions: ["3.11", "3.12"]
       tools: ["ruff", "pytest", "mypy"]
     rust:
       toolchain: "stable"
       components: ["clippy", "rustfmt"]
     node:
       version: "20.11.0"
       global_packages: ["typescript", "prettier"]

   system_tools:
     - docker
     - postgresql-client
     - redis-tools

   api_clients:
     - grpcurl
     - websocat
   ```

4. **Installation orchestration:**
   ```bash
   #!/bin/bash
   # install-from-manifest.sh

   # Parse manifest and install tools on-demand
   # Check if tool exists, install if missing
   # Track installed versions for reproducibility
   ```

**Pattern benefits:**
- Minimal base image size
- Fast startup (common tools pre-installed)
- Project-specific versions via mise
- Declarative requirements
- Auditable installation history

---

## 11. Language-Specific Tooling

### Node.js/JavaScript

**fnm - Fast Node Manager**
- **Installation:**
  ```bash
  curl -fsSL https://fnm.vercel.app/install | bash
  ```
- **Advantages over nvm:**
  - Written in Rust (faster)
  - Cross-platform parity
  - Supports `.node-version` and `.nvmrc`
- **Usage:**
  ```bash
  fnm install 20
  fnm use 20
  fnm install --lts
  ```

**Bun - JavaScript runtime/toolkit**
- **Installation:**
  ```bash
  curl -fsSL https://bun.sh/install | bash
  ```
- **Use cases:**
  - Faster than Node.js for scripts
  - Built-in bundler, test runner, package manager
  - TypeScript/JSX native support
- **When to use:**
  - New projects prioritizing speed
  - Development environments
  - Single binary deployment

**Deno - Secure JavaScript runtime**
- **Installation:**
  ```bash
  curl -fsSL https://deno.land/install.sh | sh
  ```
- **Advantages:**
  - Secure by default (explicit permissions)
  - Native TypeScript
  - Modern standard library
- **When to use:**
  - Security-first requirements
  - TypeScript-primary projects
  - No npm ecosystem dependency

**Recommendation:**
- **Default:** Node.js via fnm (ecosystem maturity)
- **Performance:** Bun for scripts/tools
- **Security:** Deno for restricted environments

### Go

**Installation (official binaries):**
```bash
GO_VERSION=1.22.0
wget https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz
sudo tar -C /usr/local -xzf go${GO_VERSION}.linux-amd64.tar.gz
rm go${GO_VERSION}.linux-amd64.tar.gz

# Add to PATH
echo 'export PATH=$PATH:/usr/local/go/bin' >> ~/.bashrc
echo 'export PATH=$PATH:$(go env GOPATH)/bin' >> ~/.bashrc
```

**Prefer official binaries over apt:**
- Newer versions than Ubuntu repos
- Consistent across distributions
- Easier version management

**Common tools:**
```bash
# Language server
go install golang.org/x/tools/gopls@latest

# Linter
go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest

# Test coverage
go install github.com/axw/gocov/gocov@latest
```

---

## 12. Pre-Install vs On-Demand Decision Matrix

### Critical Pre-Installs (Always)

**System essentials:**
- `build-essential`
- `git`
- `curl`, `wget`
- `ca-certificates`
- `gnupg`, `lsb-release`

**Core development:**
- `jq`, `yq` (data processing)
- `ripgrep`, `fd-find` (search)
- `gh` (GitHub automation)
- `docker` (containerization)
- `direnv` (environment management)

**Python:**
- `uv` (all-in-one tool)
- `ruff` (linting/formatting)
- System Python 3.12

**Debugging:**
- `strace`
- `vim` or `nano` (text editing)

### Conditional Pre-Installs (Project-Dependent)

**Rust projects:**
- `rustup`, `cargo`

**Node.js projects:**
- `fnm`, Node.js

**Database work:**
- `postgresql-client`, `mysql-client`, `redis-tools`

**gRPC:**
- `grpcurl`

**WebSocket:**
- `websocat`

**Build systems:**
- `cmake`, `meson`, `ninja-build`

### On-Demand Only (Install When Needed)

**Interactive tools:**
- `starship`, `zoxide`, `lazygit`, `delta`
- `btop`, `bottom`, `htop`

**TUI database clients:**
- `pgcli`, `mycli`, `litecli`

**Language-specific:**
- Additional Python versions (via uv/pyenv)
- Rust cross-compilation targets
- Go versions (via go install)

**Specialized:**
- `ltrace`, `perf`
- `podman`, `buildah` (if not using Docker)

### Never Install (Incompatible with Agents)

- GUI applications
- Desktop environment tools
- Screen savers, themes
- Interactive TUIs requiring human input

---

## 13. Complete Installation Script

### Minimal Base Environment

```bash
#!/bin/bash
set -euo pipefail

# Non-interactive setup
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a

echo "==> Updating package lists"
apt-get update

echo "==> Installing system essentials"
apt-get install -y \
  build-essential \
  git \
  curl \
  wget \
  ca-certificates \
  gnupg \
  lsb-release \
  software-properties-common \
  apt-transport-https

echo "==> Installing core CLI tools"
apt-get install -y \
  jq \
  ripgrep \
  fd-find \
  bat \
  strace \
  vim \
  tmux

echo "==> Installing GitHub CLI"
apt-get install -y gh

echo "==> Installing direnv"
apt-get install -y direnv

echo "==> Installing database clients"
apt-get install -y \
  postgresql-client-16 \
  mysql-client \
  redis-tools \
  sqlite3

echo "==> Installing Docker"
# Add Docker GPG key
install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg \
  -o /etc/apt/keyrings/docker.asc
chmod a+r /etc/apt/keyrings/docker.asc

# Add repository
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] \
  https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  tee /etc/apt/sources.list.d/docker.list > /dev/null

apt-get update
apt-get install -y \
  docker-ce \
  docker-ce-cli \
  containerd.io \
  docker-buildx-plugin \
  docker-compose-plugin

echo "==> Installing Python tools (uv)"
curl -LsSf https://astral.sh/uv/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"

# Install ruff
uv tool install ruff

echo "==> Installing Rust"
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
  sh -s -- -y --default-toolchain stable --profile minimal
source "$HOME/.cargo/env"
rustup component add clippy rustfmt rust-analyzer

echo "==> Installing mise"
curl https://mise.run | sh
export PATH="$HOME/.local/bin:$PATH"

echo "==> Installing build tools"
apt-get install -y \
  cmake \
  ninja-build \
  meson

echo "==> Creating symlinks"
mkdir -p ~/.local/bin
ln -sf /usr/bin/batcat ~/.local/bin/bat
ln -sf /usr/bin/fdfind ~/.local/bin/fd

echo "==> Cleaning up"
apt-get clean
rm -rf /var/lib/apt/lists/*

echo "==> Installation complete"
echo "Please log out and back in for group changes to take effect"
```

### Environment Setup Script

```bash
#!/bin/bash
# ~/.bashrc additions for agent environment

# PATH additions
export PATH="$HOME/.local/bin:$PATH"
export PATH="$HOME/.cargo/bin:$PATH"

# Rust
source "$HOME/.cargo/env"

# mise
eval "$(mise activate bash)"

# direnv
eval "$(direnv hook bash)"

# uv
export UV_CACHE_DIR="$HOME/.cache/uv"

# Go (if installed)
export PATH="$PATH:/usr/local/go/bin:$(go env GOPATH)/bin"

# Editor
export EDITOR=vim

# Non-interactive settings
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a

# Aliases
alias bat='batcat'
alias fd='fdfind'
```

---

## 14. Summary Recommendations

### For Headless Agent Environments

**Philosophy:**
1. Pre-install critical, frequently-used tools
2. Use mise/asdf for language version management
3. Skip interactive TUI tools
4. Prefer declarative manifests over imperative scripts
5. Optimize for fast startup and minimal bloat

**Tier 1 (Always Pre-install):**
- System: build-essential, git, curl, jq
- Search: ripgrep, fd-find
- Python: uv, ruff
- Containers: docker
- Git: gh
- Environment: direnv
- Debugging: strace

**Tier 2 (Conditional Pre-install):**
- Rust: rustup (if Rust projects)
- Node: fnm + Node.js (if JS projects)
- Databases: postgresql-client, mysql-client (if database work)
- Build: cmake, ninja, meson (if C/C++ projects)
- gRPC: grpcurl (if gRPC services)

**Tier 3 (On-Demand Installation):**
- Language versions (via mise)
- Cross-compilation targets
- Specialized debugging tools
- Project-specific tools

**Never Install:**
- Interactive TUIs (lazygit, delta, starship, zoxide)
- GUI applications
- Visual system monitors (btop, bottom)

### Version Management Strategy

**Use mise for:**
- Python versions (beyond system)
- Node.js versions
- Go versions
- Ruby, Java, etc.

**Use rustup for:**
- Rust toolchains
- Rust components

**Use docker for:**
- Service dependencies (databases, Redis, etc.)
- Isolated build environments

### Key Patterns

**1. Non-interactive installations:**
```bash
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -o Dpkg::Options::="--force-confnew" package
```

**2. Static binary downloads:**
```bash
curl -L URL -o ~/.local/bin/tool && chmod +x ~/.local/bin/tool
```

**3. Curl installer pattern:**
```bash
curl -fsSL https://example.com/install.sh | sh
```

**4. Version pinning:**
```toml
# mise.toml
[tools]
python = "3.12.1"  # Exact version
node = "20"        # Latest 20.x
terraform = "latest"
```

**5. Environment isolation:**
```bash
# .envrc (direnv)
export DATABASE_URL=postgres://localhost/dev
layout python python3.12
```

---

## 15. References

### Official Documentation
- **Python uv:** https://github.com/astral-sh/uv
- **Rust rustup:** https://rustup.rs/
- **Docker:** https://docs.docker.com/engine/install/ubuntu/
- **mise:** https://mise.jdx.dev/
- **GitHub CLI:** https://cli.github.com/

### Tool Repositories
- ripgrep: https://github.com/BurntSushi/ripgrep
- fd: https://github.com/sharkdp/fd
- bat: https://github.com/sharkdp/bat
- Ruff: https://github.com/astral-sh/ruff
- direnv: https://github.com/direnv/direnv

### Package Indexes
- Ubuntu packages: https://packages.ubuntu.com/noble/
- Rust crates: https://crates.io/
- PyPI: https://pypi.org/
- npm: https://www.npmjs.com/

### Version Managers
- asdf: https://github.com/asdf-vm/asdf
- pyenv: https://github.com/pyenv/pyenv
- fnm: https://github.com/Schniz/fnm

---

## Changelog

- **2026-01-28:** Initial research for Ubuntu 24.04 LTS

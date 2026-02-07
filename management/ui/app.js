/**
 * Agentic Sandbox Control Plane
 * Per-agent pane dashboard with independent output tracking
 */

const OAUTH_PATTERNS = [
    /https:\/\/[a-z0-9.-]*\.anthropic\.com\/[^\s"'<>]+/gi,
    /https:\/\/console\.anthropic\.com\/[^\s"'<>]+/gi,
    /https:\/\/github\.com\/login\/oauth\/authorize\?[^\s"'<>]+/gi,
    /https:\/\/github\.com\/login\/device[^\s"'<>]*/gi,
    /https:\/\/accounts\.google\.com\/o\/oauth2\/[^\s"'<>]+/gi,
    /https:\/\/login\.microsoftonline\.com\/[^\s"'<>]+/gi,
    /(?:open|visit|go to|navigate to|click|authorize at)[:\s]+["']?(https?:\/\/[^\s"'<>]+)/gi,
    /(?:please|now)\s+(?:open|visit|go to)[:\s]+["']?(https?:\/\/[^\s"'<>]+)/gi,
];

const DEVICE_CODE_PATTERNS = [
    /enter(?:ing)?\s+(?:the\s+)?code[:\s]+([A-Z0-9]{4,}-?[A-Z0-9]{4,})/gi,
    /user\s*code[:\s]+([A-Z0-9]{4,}-?[A-Z0-9]{4,})/gi,
];

class AgenticDashboard {
    constructor() {
        this.ws = null;
        this.agents = new Map();         // agentId -> agent info
        this.panes = new Map();          // agentId -> DOM elements
        this.activeCommandIds = new Map(); // agentId -> last command_id
        this.shellCommandIds = new Map();  // agentId -> shell session command_id
        this.reconnectAttempts = 0;
        this.maxReconnectAttempts = 10;
        this.reconnectDelay = 1000;
        this.currentOAuthPrompt = null;

        // Log sidebar state
        this.logEvents = [];
        this.systemLogs = [];            // System log messages
        this.maxLogEvents = 100;  // Limit UI to 100 events
        this.maxSystemLogs = 200;
        this.eventFilter = 'all';
        this.autoScroll = true;
        this.lastEventId = 0;  // For change detection

        // VM list state
        this.vms = new Map();  // vm_name -> VM info

        // Selected agent for single-pane display
        this.selectedAgent = null;

        // Sessions blade state
        this.selectedVmForSessions = null; // Which VM's sessions are shown
        this.vmSessions = new Map(); // vmName -> sessions array
        this.lastSelectedSession = new Map(); // vmName -> last selected session command_id

        // Per-session output buffers for live thumbnails
        // command_id -> { lines: string[], dirty: bool }
        this.sessionBuffers = new Map();
        this.maxSessionBufferLines = 50;

        this.init();
    }

    init() {
        this.setupGlobalListeners();
        this.setupLogSidebar();
        this.setupBladeNav();
        this.connect();
        this.fetchAgents();
        this.fetchEvents();
        this.fetchVms();
        this.fetchSystemLogs();

        // Refresh session thumbnails every second
        setInterval(() => this.updateSessionThumbs(), 1000);
    }

    // =========================================================================
    // WebSocket
    // =========================================================================

    connect() {
        const wsPort = parseInt(window.location.port || '8122') - 1;
        const wsUrl = `ws://${window.location.hostname}:${wsPort}`;
        console.log(`Connecting to WebSocket at ${wsUrl}`);

        try {
            this.ws = new WebSocket(wsUrl);
            this.ws.onopen = () => this.onOpen();
            this.ws.onmessage = (e) => this.onMessage(e);
            this.ws.onclose = (e) => this.onClose(e);
            this.ws.onerror = (e) => console.error('WebSocket error:', e);
        } catch (error) {
            console.error('WebSocket connection failed:', error);
            this.scheduleReconnect();
        }
    }

    onOpen() {
        console.log('WebSocket connected');
        this.reconnectAttempts = 0;
        this.updateConnectionStatus(true);
        this.send({ type: 'subscribe', agent_id: '*' });
        this.send({ type: 'list_agents' });
    }

    onMessage(event) {
        try {
            const msg = JSON.parse(event.data);
            this.handleMessage(msg);
        } catch (e) {
            console.error('Failed to parse message:', e);
        }
    }

    onClose(event) {
        console.log('WebSocket closed:', event.code);
        this.updateConnectionStatus(false);
        this.scheduleReconnect();
    }

    scheduleReconnect() {
        if (this.reconnectAttempts >= this.maxReconnectAttempts) {
            this.showToast('Connection lost. Refresh the page.', 'error');
            return;
        }
        const delay = this.reconnectDelay * Math.pow(2, this.reconnectAttempts);
        this.reconnectAttempts++;
        console.log(`Reconnecting in ${delay}ms (attempt ${this.reconnectAttempts})`);
        setTimeout(() => this.connect(), delay);
    }

    send(msg) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(msg));
        }
    }

    // =========================================================================
    // Message dispatch
    // =========================================================================

    handleMessage(msg) {
        switch (msg.type) {
            case 'output':
                this.handleOutput(msg);
                break;
            case 'agent_list':
                this.handleAgentList(msg);
                break;
            case 'metrics_update':
                this.handleMetricsUpdate(msg);
                break;
            case 'command_started':
                this.activeCommandIds.set(msg.agent_id, msg.command_id);
                break;
            case 'shell_started':
                this.handleShellStarted(msg);
                break;
            case 'subscribed':
            case 'unsubscribed':
            case 'pong':
                break;
            case 'input_sent':
                break;
            case 'vm_event':
                this.handleVmEvent(msg);
                break;
            case 'system_log':
                this.handleSystemLog(msg);
                break;
            case 'session_list':
                this.handleSessionsList(msg);
                break;
            case 'session_attached':
                // Server confirmed attach — update command_id in case client was slightly off
                if (msg.command_id && msg.agent_id) {
                    const entry = this.panes.get(msg.agent_id);
                    if (entry) {
                        const oldSession = entry.attachedSession;
                        entry.attachedSession = msg.command_id;
                        this.shellCommandIds.set(msg.agent_id, msg.command_id);
                        // If server returned different command_id, replay that buffer
                        if (oldSession !== msg.command_id && entry.term) {
                            entry.term.clear();
                            const buf = this.sessionBuffers.get(msg.command_id);
                            if (buf && buf.raw) {
                                entry.term.write(buf.raw);
                            }
                        }
                    }
                }
                break;
            case 'session_detached':
                break;
            case 'session_created':
                this.handleSessionCreated(msg);
                break;
            case 'session_killed':
                this.showToast(`Session ${msg.session_name || msg.session_id?.slice(0, 8)} killed`, 'success');
                // Refresh sessions blade if showing this agent
                if (msg.agent_id && this.selectedVmForSessions === msg.agent_id) {
                    this.fetchSessionsForBlade(msg.agent_id);
                }
                break;
            case 'reconciliation_triggered':
                this.showToast(`Reconciliation started for ${msg.agent_id}`, 'success');
                break;
            case 'error':
                console.error('Server error:', msg.message);
                this.showToast(msg.message, 'error');
                break;
            default:
                console.log('Unknown message:', msg.type, msg);
        }
    }

    handleOutput(msg) {
        // Always buffer output per command_id for session thumbnails and replay
        if (msg.command_id) {
            let buf = this.sessionBuffers.get(msg.command_id);
            if (!buf) {
                buf = { text: '', raw: '', dirty: true };
                this.sessionBuffers.set(msg.command_id, buf);
            }
            // Store raw output for replay (keep last ~32KB)
            buf.raw += msg.data;
            if (buf.raw.length > 32768) {
                buf.raw = buf.raw.slice(-32768);
            }
            // Accumulate stripped text for thumbnails
            buf.text += this.stripAnsi(msg.data);
            // Limit buffer size (keep last ~4KB)
            if (buf.text.length > 4096) {
                buf.text = buf.text.slice(-4096);
            }
            buf.dirty = true;
        }

        // Only write to main terminal if this is the attached session (or default shell)
        const entry = this.panes.get(msg.agent_id);
        const attachedId = entry?.attachedSession;
        const shellId = this.shellCommandIds.get(msg.agent_id);

        // Show in main terminal if:
        //  - No explicit session attached and this is the shell session, OR
        //  - This command_id matches the attached session
        if ((!attachedId && msg.command_id === shellId) || msg.command_id === attachedId) {
            this.appendToPane(msg.agent_id, msg.stream, msg.data, msg.ts);
        }

        this.detectOAuth(msg.agent_id, msg.command_id, msg.data);
    }

    handleMetricsUpdate(msg) {
        const entry = this.panes.get(msg.agent_id);
        if (!entry) return;

        const cpuEl = entry.pane.querySelector('.stat-cpu .stat-value');
        const memEl = entry.pane.querySelector('.stat-mem .stat-value');
        const diskEl = entry.pane.querySelector('.stat-disk .stat-value');

        if (cpuEl) {
            const cpu = msg.cpu_percent;
            cpuEl.textContent = `${cpu.toFixed(0)}%`;
            cpuEl.parentElement.className = `stat stat-cpu ${this.statLevel(cpu)}`;
        }

        if (memEl && msg.memory_total_bytes > 0) {
            const memPct = (msg.memory_used_bytes / msg.memory_total_bytes) * 100;
            const memMB = Math.round(msg.memory_used_bytes / 1024 / 1024);
            const totalMB = Math.round(msg.memory_total_bytes / 1024 / 1024);
            memEl.textContent = `${memMB}/${totalMB}M`;
            memEl.parentElement.className = `stat stat-mem ${this.statLevel(memPct)}`;
        }

        if (diskEl && msg.disk_total_bytes > 0) {
            const diskPct = (msg.disk_used_bytes / msg.disk_total_bytes) * 100;
            const diskGB = (msg.disk_used_bytes / 1024 / 1024 / 1024).toFixed(1);
            const totalGB = (msg.disk_total_bytes / 1024 / 1024 / 1024).toFixed(0);
            diskEl.textContent = `${diskGB}/${totalGB}G`;
            diskEl.parentElement.className = `stat stat-disk ${this.statLevel(diskPct)}`;
        }

        // Store system info for tooltip
        if (msg.os || msg.cpu_cores) {
            const agent = this.agents.get(msg.agent_id);
            if (agent) {
                agent._sysinfo = {
                    os: msg.os, kernel: msg.kernel,
                    cpu_cores: msg.cpu_cores,
                    uptime: msg.uptime_seconds,
                    load_avg: msg.load_avg,
                };
            }
        }
    }

    statLevel(pct) {
        if (pct >= 85) return 'stat-critical';
        if (pct >= 60) return 'stat-warning';
        return 'stat-ok';
    }

    handleVmEvent(msg) {
        // Add event to log sidebar
        const event = {
            event_type: msg.event_type,
            vm_name: msg.vm_name,
            timestamp: msg.timestamp || new Date().toISOString(),
            details: msg.details || {},
        };
        this.addEvent(event);

        // Show toast for important events
        if (msg.event_type === 'vm.crashed') {
            this.showToast(`VM ${msg.vm_name} crashed!`, 'error');
        } else if (msg.event_type === 'vm.started') {
            this.showToast(`VM ${msg.vm_name} started`, 'success');
        }

        // Refresh VM list after events
        setTimeout(() => this.fetchVms(), 500);
    }

    handleAgentList(msg) {
        if (!msg.agents) return;

        const currentIds = new Set(this.agents.keys());
        const incomingIds = new Set(msg.agents.map(a => a.id));

        // Add or update agents
        msg.agents.forEach(agent => {
            this.agents.set(agent.id, agent);
            if (!this.panes.has(agent.id)) {
                this.createPane(agent);
            } else {
                this.updatePaneHeader(agent);
            }
            // Populate metrics from REST API data (if present)
            if (agent.metrics) {
                this.handleMetricsUpdate({
                    agent_id: agent.id,
                    ...agent.metrics,
                });
            }
        });

        // Remove panes for disconnected agents
        for (const id of currentIds) {
            if (!incomingIds.has(id)) {
                this.agents.delete(id);
                this.removePane(id);
            }
        }

        this.updateAgentCount();
        this.updateEmptyState();
    }

    // =========================================================================
    // Shell management
    // =========================================================================

    startShell(agentId) {
        const entry = this.panes.get(agentId);
        if (!entry) return;

        const cols = entry.term.cols || 80;
        const rows = entry.term.rows || 24;

        console.log(`Starting shell on ${agentId} (${cols}x${rows})`);
        this.send({
            type: 'start_shell',
            agent_id: agentId,
            cols: cols,
            rows: rows,
        });
    }

    handleShellStarted(msg) {
        const { agent_id, command_id } = msg;
        this.shellCommandIds.set(agent_id, command_id);
        this.activeCommandIds.set(agent_id, command_id);
        console.log(`Shell started on ${agent_id}: ${command_id}`);

        // Focus the terminal and send resize after tmux has time to initialize
        const entry = this.panes.get(agent_id);
        if (entry && entry.term) {
            entry.term.focus();
            // Delay resize slightly to ensure tmux session is ready
            setTimeout(() => {
                // Re-fit to get accurate dimensions
                try { entry.fitAddon.fit(); } catch (_) {}
                this.send({
                    type: 'pty_resize',
                    agent_id: agent_id,
                    command_id: command_id,
                    cols: entry.term.cols || 80,
                    rows: entry.term.rows || 24,
                });
            }, 100);
        }

        // Update shell button state
        this.updateShellButton(agent_id, true);
    }

    updateShellButton(agentId, active) {
        const entry = this.panes.get(agentId);
        if (!entry) return;
        const btn = entry.pane.querySelector('.pane-shell-btn');
        if (btn) {
            btn.classList.toggle('active', active);
        }
    }

    // =========================================================================
    // Pane management
    // =========================================================================

    createPane(agent) {
        console.log('createPane called for agent:', agent.id);
        const container = document.getElementById('pane-container');
        const pane = document.createElement('div');
        pane.className = 'agent-pane';
        pane.dataset.agentId = agent.id;

        // Auto-select first agent if none selected
        if (!this.selectedAgent) {
            this.selectedAgent = agent.id;
            console.log('Auto-selected agent:', agent.id);
        }

        // Hide pane if not the selected agent
        if (this.selectedAgent !== agent.id) {
            pane.style.display = 'none';
        }

        const statusClass = agent.status.toLowerCase().replace('agent_status_', '');

        pane.innerHTML = `
            <div class="pane-header">
                <div class="pane-header-left">
                    <span class="pane-status-dot ${statusClass}"></span>
                    <span class="pane-agent-name">${this.esc(agent.id)}</span>
                    <span class="pane-agent-host">${this.esc(agent.hostname || agent.ip_address || '')}</span>
                </div>
                <div class="pane-stats">
                    <span class="stat stat-cpu" title="CPU"><span class="stat-label">CPU</span> <span class="stat-value">--</span></span>
                    <span class="stat stat-mem" title="Memory"><span class="stat-label">MEM</span> <span class="stat-value">--</span></span>
                    <span class="stat stat-disk" title="Disk"><span class="stat-label">DSK</span> <span class="stat-value">--</span></span>
                </div>
                <div class="pane-controls">
                    <button class="pane-vm-btn pane-vm-restart" title="Restart VM" data-action="restart">&#10227;</button>
                    <button class="pane-vm-btn pane-vm-stop" title="Stop VM" data-action="stop">&#9208;</button>
                    <button class="pane-vm-btn pane-vm-kill" title="Force Kill VM" data-action="destroy">&#9209;</button>
                    <button class="pane-shell-btn" title="Reconnect to tmux session">Reconnect</button>
                </div>
            </div>
            <div class="pane-output"></div>
        `;

        const outputEl = pane.querySelector('.pane-output');
        const shellBtn = pane.querySelector('.pane-shell-btn');

        // VM control buttons
        const restartBtn = pane.querySelector('.pane-vm-restart');
        const stopBtn = pane.querySelector('.pane-vm-stop');
        const killBtn = pane.querySelector('.pane-vm-kill');

        restartBtn.addEventListener('click', () => this.handleVmControl(agent.id, 'restart'));
        stopBtn.addEventListener('click', () => this.handleVmControl(agent.id, 'stop'));
        killBtn.addEventListener('click', () => this.handleVmControl(agent.id, 'destroy'));

        // Initialize xterm.js terminal — stdin enabled for PTY.
        // scrollback: 0 because tmux manages its own scrollback buffer.
        // This also eliminates the xterm scrollbar, giving FitAddon an
        // accurate column calculation (no scrollbar width to estimate).
        const term = new Terminal({
            cursorBlink: true,
            cursorStyle: 'block',
            disableStdin: false,
            convertEol: false,
            scrollback: 0,
            fontSize: 13,
            fontFamily: "'SF Mono', 'Fira Code', 'Consolas', monospace",
            theme: {
                background: '#0d0d1a',
                foreground: '#00ff88',
                cursor: '#00ff88',
                black: '#0d0d1a',
                red: '#ff4444',
                green: '#00ff88',
                yellow: '#ffaa00',
                blue: '#00d9ff',
                magenta: '#7b2cbf',
                cyan: '#00d9ff',
                white: '#e8e8e8',
            },
        });

        // Fit addon — auto-resize terminal to container
        const fitAddon = new FitAddon.FitAddon();
        term.loadAddon(fitAddon);

        container.appendChild(pane);

        // Wrapper div so FitAddon measures the inset area, not the full container.
        // Without this, tmux status bar overflows because FitAddon calculates
        // more columns than visually fit inside the padded region.
        const xtermWrapper = document.createElement('div');
        xtermWrapper.className = 'xterm-wrapper';
        outputEl.appendChild(xtermWrapper);
        term.open(xtermWrapper);

        // Fit after DOM insertion, then start shell with correct dimensions
        const self = this;
        requestAnimationFrame(() => {
            try { fitAddon.fit(); } catch (_) {}
            // Start shell after fit so PTY gets correct size
            self.startShell(agent.id);
        });

        // Re-fit on window resize and send PTY resize
        const resizeObserver = new ResizeObserver(() => {
            try {
                fitAddon.fit();
                // Send resize to PTY if shell is active
                const shellCmdId = this.shellCommandIds.get(agent.id);
                if (shellCmdId && term.cols && term.rows) {
                    this.send({
                        type: 'pty_resize',
                        agent_id: agent.id,
                        command_id: shellCmdId,
                        cols: term.cols,
                        rows: term.rows,
                    });
                }
            } catch (_) {}
        });
        resizeObserver.observe(xtermWrapper);

        // Ensure control keys (Ctrl+C, etc.) go to the PTY, not the browser.
        // Exception: allow browser Ctrl+C (copy) when text is selected.
        term.attachCustomKeyEventHandler((ev) => {
            if (ev.type !== 'keydown') return true;
            if (ev.ctrlKey && ev.key === 'c' && term.hasSelection()) {
                return false; // let browser copy selection
            }
            if (ev.ctrlKey && ev.key === 'v') {
                return false; // let browser paste
            }
            return true; // send everything else to PTY
        });

        // Forward terminal keystrokes to shell stdin
        term.onData((data) => {
            // Filter out terminal response sequences (DA1, DA2, cursor position reports, etc.)
            // These are responses to queries that shouldn't be sent as PTY input
            // Match with or without escape prefix (may be stripped or chunked)
            if (data.match(/^\x1b\[\??[\d;]*[cRn]$/) ||      // ESC [ sequences
                data.match(/^[\d;]+[cRn]$/) ||               // Response without ESC prefix (chunked)
                data.match(/^\x1b\].*\x07$/) ||              // OSC sequences
                data.match(/^\x1bP.*\x1b\\$/) ||             // DCS sequences
                data.match(/^\x1b[\[\]PO]/) ||               // Any escape sequence start
                data.match(/^[0-9;]+c$/)) {                  // Bare DA response like "0;276;0c"
                // Silently drop terminal response sequences
                return;
            }

            const shellCmdId = this.shellCommandIds.get(agent.id);
            if (shellCmdId) {
                this.send({
                    type: 'send_input',
                    agent_id: agent.id,
                    command_id: shellCmdId,
                    data: data,
                });
            }
        });

        // Shell button — reconnect to tmux session (kills old PTY, starts fresh attach)
        shellBtn.addEventListener('click', () => {
            term.clear();
            term.reset();
            this.startShell(agent.id);
        });

        this.panes.set(agent.id, { pane, output: outputEl, term, fitAddon, resizeObserver });
        console.log('Pane created and stored for:', agent.id, 'Total panes:', this.panes.size, 'Keys:', [...this.panes.keys()]);
        // Shell auto-started in RAF callback above after fit completes
    }

    updatePaneHeader(agent) {
        const entry = this.panes.get(agent.id);
        if (!entry) return;
        const dot = entry.pane.querySelector('.pane-status-dot');
        const statusClass = agent.status.toLowerCase().replace('agent_status_', '');
        dot.className = `pane-status-dot ${statusClass}`;
    }

    removePane(agentId) {
        // Close sessions blade if showing this agent
        if (this.selectedVmForSessions === agentId) {
            this.closeSessionsBlade();
        }
        this.vmSessions.delete(agentId);

        const entry = this.panes.get(agentId);
        if (entry) {
            if (entry.resizeObserver) entry.resizeObserver.disconnect();
            if (entry.term) entry.term.dispose();
            entry.pane.remove();
            this.panes.delete(agentId);
        }
    }

    appendToPane(agentId, stream, data, timestamp) {
        let entry = this.panes.get(agentId);
        if (!entry) {
            // Agent not yet known — create a stub pane
            this.createPane({ id: agentId, status: 'ready', hostname: '' });
            entry = this.panes.get(agentId);
        }

        if (!entry.term) return;

        // For PTY shell output, write raw (PTY handles its own newlines/escapes)
        const shellCmdId = this.shellCommandIds.get(agentId);
        if (shellCmdId && stream === 'stdout') {
            entry.term.write(data);
            return;
        }

        // Non-PTY output: apply color prefix based on stream type
        let prefix = '';
        if (stream === 'stderr') {
            prefix = '\x1b[31m'; // red
        } else if (stream === 'log') {
            prefix = '\x1b[90m'; // dim gray
        }
        const reset = prefix ? '\x1b[0m' : '';

        const text = prefix + data + reset;
        entry.term.write(text);
    }

    // =========================================================================
    // VM Control
    // =========================================================================

    handleVmControl(agentId, action) {
        // Find VM name from agent ID (convention: agent ID matches VM name)
        const vmName = agentId;

        if (action === 'destroy') {
            this.showConfirmDialog({
                title: 'Force Kill VM?',
                message: `This will immediately terminate ${vmName}. Any unsaved work will be lost.`,
                confirmText: 'Kill',
                confirmClass: 'danger',
                onConfirm: () => this.destroyVm(vmName)
            });
        } else if (action === 'restart') {
            this.restartVm(vmName);
        } else if (action === 'stop') {
            this.stopVm(vmName);
        }
    }

    async startVm(name) {
        this.showToast(`Starting ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = await fetch(`/api/v1/vms/${name}/start`, { method: 'POST' });
            if (resp.ok) {
                this.showToast(`${name} started successfully`, 'success');
                setTimeout(() => this.fetchVms(), 1000);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to start ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Start VM error:', e);
            this.showToast(`Failed to start ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async stopVm(name) {
        this.showToast(`Shutting down and removing ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            // First gracefully stop the VM
            const stopResp = await fetch(`/api/v1/vms/${name}/stop`, { method: 'POST' });
            if (!stopResp.ok) {
                const data = await stopResp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to stop ${name}: ${data.error || stopResp.statusText}`, 'error');
                return;
            }

            // Wait briefly for shutdown to complete
            await new Promise(r => setTimeout(r, 2000));

            // Then delete the VM completely
            const deleteResp = await fetch(`/api/v1/vms/${name}?force=true&delete_disk=true`, { method: 'DELETE' });
            if (deleteResp.ok) {
                this.showToast(`${name} removed successfully`, 'success');
                setTimeout(() => this.fetchVms(), 500);
            } else {
                const data = await deleteResp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to remove ${name}: ${data.error || deleteResp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Stop VM error:', e);
            this.showToast(`Failed to stop ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async destroyVm(name) {
        this.showToast(`Force killing and removing ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            // Delete with force=true will destroy running VM first, then undefine and delete disk
            const resp = await fetch(`/api/v1/vms/${name}?force=true&delete_disk=true`, { method: 'DELETE' });
            if (resp.ok) {
                this.showToast(`${name} removed`, 'success');
                setTimeout(() => this.fetchVms(), 500);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to remove ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Destroy VM error:', e);
            this.showToast(`Failed to remove ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async restartVm(name) {
        this.showToast(`Restarting ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = await fetch(`/api/v1/vms/${name}/restart`, { method: 'POST' });
            if (resp.ok) {
                this.showToast(`${name} restarted`, 'success');
                setTimeout(() => this.fetchVms(), 1000);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to restart ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Restart VM error:', e);
            this.showToast(`Failed to restart ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    setVmButtonsDisabled(vmName, disabled) {
        // Disable buttons in agent pane
        const entry = this.panes.get(vmName);
        if (entry) {
            const buttons = entry.pane.querySelectorAll('.pane-vm-btn');
            buttons.forEach(btn => btn.disabled = disabled);
        }

        // Disable buttons in VM list
        const vmEntry = document.querySelector(`[data-vm-name="${vmName}"]`);
        if (vmEntry) {
            const buttons = vmEntry.querySelectorAll('.vm-ctrl-btn');
            buttons.forEach(btn => btn.disabled = disabled);
        }
    }

    async deleteVm(name) {
        // For stopped VMs - just delete (no force needed)
        this.showToast(`Deleting ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = await fetch(`/api/v1/vms/${name}?delete_disk=true`, { method: 'DELETE' });
            if (resp.ok) {
                this.showToast(`${name} deleted`, 'success');
                setTimeout(() => this.fetchVms(), 500);
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to delete ${name}: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Delete VM error:', e);
            this.showToast(`Failed to delete ${name}: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async deployAgent(name) {
        this.showToast(`Deploying agent to ${name}...`, 'info');
        this.setVmButtonsDisabled(name, true);

        try {
            const resp = await fetch(`/api/v1/vms/${name}/deploy-agent`, { method: 'POST' });
            if (resp.ok || resp.status === 202) {
                const data = await resp.json();
                if (data.operation) {
                    this.showToast(`Agent deployment started on ${name}`, 'success');
                    this.pollDeployOperation(data.operation.id, name);
                } else {
                    this.showToast(`Agent deployed to ${name}`, 'success');
                    setTimeout(() => this.fetchAgents(), 2000);
                }
            } else {
                const data = await resp.json().catch(() => ({ error: 'Unknown error' }));
                this.showToast(`Failed to deploy agent: ${data.error || resp.statusText}`, 'error');
            }
        } catch (e) {
            console.error('Deploy agent error:', e);
            this.showToast(`Failed to deploy agent: ${e.message}`, 'error');
        } finally {
            this.setVmButtonsDisabled(name, false);
        }
    }

    async pollDeployOperation(opId, vmName) {
        const maxAttempts = 60; // 5 minutes at 5s intervals
        let attempts = 0;

        const poll = async () => {
            try {
                const resp = await fetch(`/api/v1/operations/${opId}`);
                if (!resp.ok) return;

                const op = await resp.json();
                if (op.state === 'completed') {
                    this.showToast(`Agent deployed to ${vmName}!`, 'success');
                    this.fetchAgents();
                    return;
                } else if (op.state === 'failed') {
                    this.showToast(`Agent deployment failed: ${op.error || 'Unknown'}`, 'error');
                    return;
                }

                attempts++;
                if (attempts < maxAttempts) {
                    setTimeout(poll, 5000);
                } else {
                    this.showToast(`Deployment timed out. Check logs.`, 'warning');
                }
            } catch (e) {
                console.error('Poll deploy operation error:', e);
            }
        };

        setTimeout(poll, 3000);
    }

    // =========================================================================
    // VM List Sidebar
    // =========================================================================

    setupVmSidebar() {
        // Legacy - now handled by setupBladeNav
        this.setupCreateVmModal();
    }

    setupCreateVmModal() {
        const modal = document.getElementById('create-vm-modal');
        if (!modal) return;

        const overlay = modal.querySelector('.modal-overlay');
        const closeBtn = modal.querySelector('.modal-close');
        const cancelBtn = modal.querySelector('.cancel-btn');
        const form = document.getElementById('create-vm-form');

        // Close modal handlers
        const closeModal = () => {
            modal.classList.add('hidden');
            form.reset();
        };

        overlay.addEventListener('click', closeModal);
        closeBtn.addEventListener('click', closeModal);
        cancelBtn.addEventListener('click', closeModal);

        // Handle form submission
        form.addEventListener('submit', async (e) => {
            e.preventDefault();
            await this.handleCreateVm();
        });

        // Handle ESC key
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && !modal.classList.contains('hidden')) {
                closeModal();
            }
        });
    }

    showCreateVmModal() {
        const modal = document.getElementById('create-vm-modal');
        if (modal) {
            modal.classList.remove('hidden');
            document.getElementById('vm-name').focus();
        }
    }

    async handleCreateVm() {
        const nameInput = document.getElementById('vm-name');
        const name = `agent-${nameInput.value.trim()}`;
        const profile = document.getElementById('vm-profile').value;
        const vcpus = parseInt(document.getElementById('vm-vcpus').value);
        const memory_mb = parseInt(document.getElementById('vm-memory').value);
        const disk_gb = parseInt(document.getElementById('vm-disk').value);
        const agentshare = document.getElementById('vm-agentshare').checked;
        const start = document.getElementById('vm-autostart').checked;

        // Validate name
        if (!nameInput.value.trim()) {
            this.showToast('Please enter a VM name', 'error');
            return;
        }

        if (!/^[a-z0-9-]+$/.test(nameInput.value)) {
            this.showToast('Name can only contain lowercase letters, numbers, and hyphens', 'error');
            return;
        }

        // Close modal
        document.getElementById('create-vm-modal').classList.add('hidden');
        document.getElementById('create-vm-form').reset();

        // Show progress toast
        this.showToast(`Creating ${name}... This may take several minutes.`, 'info');

        try {
            const resp = await fetch('/api/v1/vms', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    name,
                    profile,
                    vcpus,
                    memory_mb,
                    disk_gb,
                    agentshare,
                    start
                })
            });

            if (resp.ok || resp.status === 202) {
                const data = await resp.json();
                if (data.operation) {
                    this.showToast(`${name} provisioning started. Operation: ${data.operation.id}`, 'success');
                    // Poll for operation status
                    this.pollOperation(data.operation.id, name);
                } else {
                    this.showToast(`${name} created successfully`, 'success');
                    setTimeout(() => this.fetchVms(), 2000);
                }
            } else {
                const data = await resp.json().catch(() => ({}));
                const msg = data.error?.message || data.error || resp.statusText;
                this.showToast(`Failed to create ${name}: ${msg}`, 'error');
            }
        } catch (e) {
            console.error('Create VM error:', e);
            this.showToast(`Failed to create ${name}: ${e.message}`, 'error');
        }
    }

    async pollOperation(opId, vmName) {
        const maxAttempts = 120; // 10 minutes at 5s intervals
        let attempts = 0;

        const poll = async () => {
            try {
                const resp = await fetch(`/api/v1/operations/${opId}`);
                if (!resp.ok) {
                    console.error('Failed to poll operation:', resp.status);
                    return;
                }

                const op = await resp.json();

                if (op.state === 'completed') {
                    this.showToast(`${vmName} created successfully!`, 'success');
                    this.fetchVms();
                    return;
                } else if (op.state === 'failed') {
                    this.showToast(`${vmName} creation failed: ${op.error || 'Unknown error'}`, 'error');
                    return;
                }

                // Still running, poll again
                attempts++;
                if (attempts < maxAttempts) {
                    setTimeout(poll, 5000);
                } else {
                    this.showToast(`${vmName} creation timed out. Check logs.`, 'warning');
                }
            } catch (e) {
                console.error('Poll operation error:', e);
            }
        };

        // Start polling after 5 seconds
        setTimeout(poll, 5000);
    }

    async fetchVms() {
        try {
            // Only fetch agent-* VMs (default prefix filter)
            const resp = await fetch('/api/v1/vms');
            if (!resp.ok) {
                // API not implemented yet
                if (resp.status === 404) {
                    console.log('VM list API not yet implemented');
                    return;
                }
                throw new Error(`HTTP ${resp.status}`);
            }
            const data = await resp.json();
            if (data.vms) {
                this.updateVmList(data.vms);
            }
        } catch (e) {
            console.error('Failed to fetch VMs:', e);
        }
    }

    updateVmList(vms) {
        const list = document.getElementById('vm-list');
        if (!list) return;

        // Update internal state
        this.vms.clear();
        vms.forEach(vm => this.vms.set(vm.name, vm));

        // Render VM list
        if (vms.length === 0) {
            list.innerHTML = '<div class="vm-placeholder">No VMs found</div>';
            return;
        }

        list.innerHTML = vms.map(vm => this.renderVmEntry(vm)).join('');

        // Attach event listeners
        list.querySelectorAll('.blade-item').forEach(item => {
            const vmName = item.dataset.vmName;
            const vm = this.vms.get(vmName);
            if (!vm) return;

            // Main item click - open sessions blade if agent connected
            item.addEventListener('click', (e) => {
                // Ignore clicks on control buttons
                if (e.target.closest('.vm-controls')) return;

                // Check if agent is connected
                if (this.panes.has(vmName)) {
                    this.openSessionsBlade(vmName);
                } else if (vm.state !== 'running') {
                    this.showToast(`${vmName} is not running`, 'info');
                } else {
                    this.showToast(`${vmName} agent not connected`, 'info');
                }
            });

            // VM control button handlers
            item.querySelector('.vm-start')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.startVm(vmName);
            });
            item.querySelector('.vm-stop')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.stopVm(vmName);
            });
            item.querySelector('.vm-kill')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.destroyVm(vmName);
            });
            item.querySelector('.vm-delete')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.deleteVm(vmName);
            });
            item.querySelector('.vm-deploy')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.deployAgent(vmName);
            });
        });

        // Update header VM count
        this.updateVmCount();
    }

    renderVmEntry(vm) {
        const stateClass = vm.state.toLowerCase().replace(' ', '-');
        const isRunning = vm.state === 'running';
        const isStopped = vm.state === 'shut off' || vm.state === 'stopped';
        const hasAgent = this.panes.has(vm.name);
        const sessionCount = this.vmSessions.get(vm.name)?.length || 0;
        const selected = vm.name === this.selectedAgent ? 'selected' : '';

        // Status icon
        const statusIcon = isRunning ? (hasAgent ? '●' : '○') : '○';
        const statusClass = isRunning ? (hasAgent ? 'running' : '') : 'stopped';

        // Session badge
        const badgeStyle = sessionCount > 0 ? '' : 'display:none';
        const badge = hasAgent ? `<span class="blade-item-badge" style="${badgeStyle}">${sessionCount}</span>` : '';

        // VM control buttons based on state
        let vmControls = '';
        if (isRunning) {
            // Running: show deploy agent (if no agent), stop, force kill
            const deployBtn = !hasAgent
                ? `<button class="vm-ctrl-btn vm-deploy" title="Deploy Agent">⚡</button>`
                : '';
            vmControls = `
                <div class="vm-controls">
                    ${deployBtn}
                    <button class="vm-ctrl-btn vm-stop" title="Stop VM">■</button>
                    <button class="vm-ctrl-btn vm-kill" title="Force Kill">✕</button>
                </div>
            `;
        } else if (isStopped) {
            // Stopped: show start, delete
            vmControls = `
                <div class="vm-controls">
                    <button class="vm-ctrl-btn vm-start" title="Start VM">▶</button>
                    <button class="vm-ctrl-btn vm-delete" title="Delete VM">🗑</button>
                </div>
            `;
        }

        return `
            <div class="blade-item ${statusClass} ${selected}" data-vm-name="${this.esc(vm.name)}">
                <span class="blade-item-icon">${statusIcon}</span>
                <div class="blade-item-info">
                    <span class="blade-item-name">${this.esc(vm.name)}${badge}</span>
                </div>
                ${vmControls}
            </div>
        `;
    }

    getVmStateIcon(state) {
        switch (state.toLowerCase()) {
            case 'running': return '&#9679;'; // filled circle
            case 'shut off':
            case 'stopped': return '&#9675;'; // empty circle
            case 'crashed':
            case 'paused': return '&#9888;'; // warning triangle
            default: return '&#9676;'; // dotted circle
        }
    }

    focusAgentPane(agentId) {
        console.log('focusAgentPane called with:', agentId);
        const entry = this.panes.get(agentId);
        if (!entry || !entry.pane) {
            console.log('Entry not found or no pane:', entry);
            return;
        }

        // Update selected agent
        this.selectedAgent = agentId;
        console.log('Switching panes, total panes:', this.panes.size);

        // Hide all panes, show selected
        this.panes.forEach((e, id) => {
            if (e.pane) {
                const display = id === agentId ? 'flex' : 'none';
                console.log(`  Pane ${id}: display=${display}`);
                e.pane.style.display = display;
            }
        });

        // Update VM list selection highlight
        document.querySelectorAll('#vm-list .blade-item').forEach(el => {
            el.classList.toggle('selected', el.dataset.vmName === agentId);
        });

        // Focus terminal and re-fit
        if (entry.term) {
            entry.term.focus();
            // Re-fit after display change
            requestAnimationFrame(() => {
                try { entry.fitAddon.fit(); } catch (_) {}
            });
        }

        // Open sessions blade for this agent
        this.openSessionsBlade(agentId);
    }

    // =========================================================================
    // Confirmation Dialog
    // =========================================================================

    showConfirmDialog({ title, message, confirmText, confirmClass, onConfirm }) {
        const modal = document.getElementById('confirm-modal');
        if (!modal) {
            console.error('Confirm modal not found');
            return;
        }

        modal.querySelector('.confirm-title').textContent = title;
        modal.querySelector('.confirm-message').textContent = message;

        const confirmBtn = modal.querySelector('.confirm-btn');
        confirmBtn.textContent = confirmText;
        confirmBtn.className = `confirm-btn ${confirmClass}`;

        // Set up event handlers
        const handleConfirm = () => {
            onConfirm();
            this.hideConfirmDialog();
        };

        const handleCancel = () => {
            this.hideConfirmDialog();
        };

        // Remove old listeners
        const newConfirmBtn = confirmBtn.cloneNode(true);
        confirmBtn.parentNode.replaceChild(newConfirmBtn, confirmBtn);

        const cancelBtn = modal.querySelector('.cancel-btn');
        const newCancelBtn = cancelBtn.cloneNode(true);
        cancelBtn.parentNode.replaceChild(newCancelBtn, cancelBtn);

        // Attach new listeners
        newConfirmBtn.addEventListener('click', handleConfirm);
        newCancelBtn.addEventListener('click', handleCancel);

        // Show modal
        modal.classList.remove('hidden');
    }

    hideConfirmDialog() {
        const modal = document.getElementById('confirm-modal');
        if (modal) {
            modal.classList.add('hidden');
        }
    }

    // =========================================================================
    // UI helpers
    // =========================================================================

    updateConnectionStatus(connected) {
        const el = document.getElementById('connection-status');
        const text = el.querySelector('.status-text');
        if (connected) {
            el.className = 'status-connected';
            text.textContent = 'Connected';
        } else {
            el.className = 'status-disconnected';
            text.textContent = 'Disconnected';
        }
    }

    updateAgentCount() {
        document.getElementById('agent-count').textContent =
            `${this.agents.size} agent${this.agents.size !== 1 ? 's' : ''}`;
    }

    updateVmCount() {
        const vmCountEl = document.getElementById('vm-count');
        if (vmCountEl) {
            const total = this.vms.size;
            const running = Array.from(this.vms.values()).filter(vm =>
                vm.state === 'running' || vm.state === 'Running'
            ).length;
            vmCountEl.textContent = running === total
                ? `${total} VM${total !== 1 ? 's' : ''}`
                : `${running}/${total} VMs`;
        }
    }

    updateEmptyState() {
        const empty = document.getElementById('no-agents');
        if (empty) {
            empty.style.display = this.panes.size === 0 ? 'flex' : 'none';
        }
    }

    async fetchAgents() {
        try {
            const resp = await fetch('/api/v1/agents');
            const data = await resp.json();
            if (data.agents) {
                this.handleAgentList({ agents: data.agents });
            }
        } catch (e) {
            console.error('Failed to fetch agents:', e);
        }
    }

    // =========================================================================
    // Blade Navigation (VMs → Sessions)
    // =========================================================================

    setupBladeNav() {
        // Back button on sessions blade
        const backBtn = document.querySelector('#sessions-blade .blade-back');
        if (backBtn) {
            backBtn.addEventListener('click', () => this.closeSessionsBlade());
        }

        // Reconcile button
        const reconcileBtn = document.getElementById('reconcile-btn');
        if (reconcileBtn) {
            reconcileBtn.addEventListener('click', () => {
                if (this.selectedVmForSessions) {
                    this.triggerReconciliation(this.selectedVmForSessions);
                }
            });
        }

        // Create VM button
        const createBtn = document.getElementById('create-vm-btn');
        if (createBtn) {
            createBtn.addEventListener('click', () => this.showCreateVmModal());
        }

        // Create Session button
        const createSessionBtn = document.getElementById('create-session-btn');
        if (createSessionBtn) {
            createSessionBtn.addEventListener('click', () => this.showCreateSessionModal());
        }

        this.setupCreateVmModal();
        this.setupCreateSessionModal();
    }

    openSessionsBlade(vmName) {
        this.selectedVmForSessions = vmName;

        const blade = document.getElementById('sessions-blade');
        const title = document.getElementById('sessions-blade-title');

        if (blade) {
            blade.classList.remove('hidden');
            blade.classList.remove('closing');
        }
        if (title) {
            title.textContent = vmName;
        }

        // Show loading
        const list = document.getElementById('sessions-list');
        if (list) {
            list.innerHTML = '<div class="blade-loading">Loading...</div>';
        }

        this.fetchSessionsForBlade(vmName);
    }

    closeSessionsBlade() {
        const blade = document.getElementById('sessions-blade');
        if (blade) {
            blade.classList.add('closing');
            setTimeout(() => {
                blade.classList.add('hidden');
                blade.classList.remove('closing');
            }, 150);
        }
        this.selectedVmForSessions = null;
    }

    fetchSessionsForBlade(vmName) {
        this.send({
            type: 'list_sessions',
            agent_id: vmName
        });
    }

    handleSessionsList(msg) {
        const vmName = msg.agent_id;
        if (!vmName) return;

        const sessions = msg.sessions || [];
        this.vmSessions.set(vmName, sessions);

        // Update blade if showing this VM
        if (this.selectedVmForSessions === vmName) {
            this.renderSessionsBlade(sessions);
        }

        // Update VM list badge
        this.updateVmSessionBadge(vmName, sessions.length);
    }

    updateVmSessionBadge(vmName, count) {
        const badge = document.querySelector(`.blade-item[data-vm-name="${vmName}"] .blade-item-badge`);
        if (badge) {
            badge.textContent = count;
            badge.style.display = count > 0 ? '' : 'none';
        }
    }

    renderSessionsBlade(sessions) {
        const list = document.getElementById('sessions-list');
        if (!list) return;

        if (!sessions || sessions.length === 0) {
            list.innerHTML = '<div class="blade-placeholder">No active sessions</div>';
            return;
        }

        list.innerHTML = sessions.map(session => {
            const typeClass = (session.session_type || 'background').toLowerCase();
            const name = session.session_name || session.command_id?.slice(0, 12) || 'session';

            // Pre-populate thumbnail from existing buffer if available
            const buf = this.sessionBuffers.get(session.command_id);
            const thumbText = buf ? this.esc(buf.text.split('\n').slice(-6).join('\n')) : '';

            return `
                <div class="session-card" data-session-id="${this.esc(session.command_id)}">
                    <div class="session-thumb" data-command-id="${this.esc(session.command_id)}">
                        <pre class="thumb-term">${thumbText}</pre>
                    </div>
                    <div class="session-card-info">
                        <span class="session-card-name">${this.esc(name)}</span>
                        <span class="session-card-type ${typeClass}">${typeClass.slice(0, 3)}</span>
                        <button class="session-card-kill" title="Kill">✕</button>
                    </div>
                </div>
            `;
        }).join('');

        // Attach handlers
        const vmName = this.selectedVmForSessions;
        list.querySelectorAll('.session-card').forEach(card => {
            const sessionId = card.dataset.sessionId;

            card.addEventListener('click', (e) => {
                if (e.target.classList.contains('session-card-kill')) return;
                this.connectToSession(vmName, sessionId);
            });

            card.querySelector('.session-card-kill')?.addEventListener('click', (e) => {
                e.stopPropagation();
                this.killSession(vmName, sessionId);
            });
        });

        // Auto-select session:
        // - If only one session, connect to it automatically
        // - If multiple sessions, try last selected, otherwise first
        if (sessions.length === 1) {
            this.connectToSession(vmName, sessions[0].command_id);
        } else if (sessions.length > 1) {
            const lastId = this.lastSelectedSession.get(vmName);
            const validLast = lastId && sessions.find(s => s.command_id === lastId);
            if (validLast) {
                this.connectToSession(vmName, lastId);
            } else {
                // Default to first session
                this.connectToSession(vmName, sessions[0].command_id);
            }
        }
    }

    connectToSession(vmName, sessionId) {
        // Find session name from our cached sessions
        const sessions = this.vmSessions.get(vmName) || [];
        const session = sessions.find(s => s.command_id === sessionId);
        const sessionName = session?.name || session?.session_name || sessionId.slice(0, 12);

        // Remember this as the last selected session for this VM
        this.lastSelectedSession.set(vmName, sessionId);

        // Make sure we have a pane for this VM and focus it
        let entry = this.panes.get(vmName);
        if (!entry) {
            // Need to focus/select this agent first
            this.focusAgentPane(vmName);
            entry = this.panes.get(vmName);
        }
        if (!entry) {
            this.showToast(`No terminal pane for ${vmName}`, 'error');
            return;
        }

        // Ensure this pane is visible/focused
        if (this.selectedAgent !== vmName) {
            this.focusAgentPane(vmName);
        }

        // Mark as active
        document.querySelectorAll('.session-card').forEach(c => c.classList.remove('active'));
        const card = document.querySelector(`.session-card[data-session-id="${sessionId}"]`);
        if (card) card.classList.add('active');

        // Get terminal size
        let cols = 80, rows = 24;
        if (entry?.term) {
            cols = entry.term.cols;
            rows = entry.term.rows;
        }

        // Send attach message and track locally for client-side output routing
        this.send({
            type: 'attach_session',
            agent_id: vmName,
            session_name: sessionName,
            cols,
            rows
        });

        // Track attached session and route input to it
        if (entry) {
            entry.attachedSession = sessionId;
            entry.attachedSessionName = sessionName;
            // Update shell command ID so keyboard input routes to this session
            this.shellCommandIds.set(vmName, sessionId);
            // Clear terminal and replay buffered output for this session
            if (entry.term) {
                entry.term.clear();
                // Replay raw buffered output from this session
                const buf = this.sessionBuffers.get(sessionId);
                if (buf && buf.raw) {
                    entry.term.write(buf.raw);
                }
                entry.term.focus();
            }
        }

        this.showToast(`Attached to ${sessionName}`, 'success');
    }

    detachSession(vmName) {
        const entry = this.panes.get(vmName);
        if (!entry?.attachedSessionName) return;

        const sessionName = entry.attachedSessionName;
        this.send({
            type: 'detach_session',
            agent_id: vmName,
            session_name: sessionName
        });

        entry.attachedSession = null;
        entry.attachedSessionName = null;

        // Clear active state from cards
        document.querySelectorAll('.session-card').forEach(c => c.classList.remove('active'));

        this.showToast('Detached from session', 'info');
    }

    killSession(vmName, commandId) {
        // Look up session_name from cached sessions (server expects session_name)
        const sessions = this.vmSessions.get(vmName) || [];
        const session = sessions.find(s => s.command_id === commandId);
        const sessionName = session?.session_name || commandId;

        console.log('killSession:', { vmName, commandId, sessions, session, sessionName });

        this.send({
            type: 'kill_session',
            agent_id: vmName,
            session_name: sessionName,
        });
        this.showToast(`Killing session "${sessionName}"...`, 'info');
    }

    triggerReconciliation(vmName) {
        this.send({
            type: 'trigger_reconciliation',
            agent_id: vmName
        });
        this.showToast(`Reconciling ${vmName}...`, 'info');
    }

    // =========================================================================
    // Create Session
    // =========================================================================

    setupCreateSessionModal() {
        const modal = document.getElementById('create-session-modal');
        if (!modal) return;

        const overlay = modal.querySelector('.modal-overlay');
        const closeBtn = modal.querySelector('.modal-close');
        const cancelBtn = modal.querySelector('.cancel-btn');
        const form = document.getElementById('create-session-form');
        const typeSelect = document.getElementById('session-type');

        const closeModal = () => {
            modal.classList.add('hidden');
            form.reset();
            this.updateSessionCommandVisibility();
        };

        overlay.addEventListener('click', closeModal);
        closeBtn.addEventListener('click', closeModal);
        cancelBtn.addEventListener('click', closeModal);

        // Show/hide command field based on session type
        typeSelect.addEventListener('change', () => this.updateSessionCommandVisibility());

        form.addEventListener('submit', (e) => {
            e.preventDefault();
            this.handleCreateSession();
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && !modal.classList.contains('hidden')) {
                closeModal();
            }
        });
    }

    updateSessionCommandVisibility() {
        const typeSelect = document.getElementById('session-type');
        const cmdGroup = document.getElementById('session-command-group');
        const cmdInput = document.getElementById('session-command');
        if (!typeSelect || !cmdGroup) return;

        const isInteractive = typeSelect.value === 'interactive';
        cmdGroup.style.display = isInteractive ? 'none' : '';
        cmdInput.required = !isInteractive;
    }

    showCreateSessionModal() {
        const vmName = this.selectedVmForSessions;
        if (!vmName) {
            this.showToast('Select a VM first', 'error');
            return;
        }

        const modal = document.getElementById('create-session-modal');
        const vmLabel = document.getElementById('session-modal-vm');
        if (vmLabel) vmLabel.textContent = vmName;

        // Reset and show
        document.getElementById('create-session-form').reset();
        this.updateSessionCommandVisibility();
        modal.classList.remove('hidden');
        document.getElementById('session-name').focus();
    }

    handleCreateSession() {
        const vmName = this.selectedVmForSessions;
        if (!vmName) return;

        const nameInput = document.getElementById('session-name');
        const name = nameInput.value.trim();
        const sessionType = document.getElementById('session-type').value;
        const workingDir = document.getElementById('session-working-dir')?.value.trim() || null;
        const commandRaw = document.getElementById('session-command').value.trim();

        if (!name) {
            this.showToast('Session name is required', 'error');
            return;
        }

        // For non-interactive types, command is required
        if (sessionType !== 'interactive' && !commandRaw) {
            this.showToast('Command is required for this session type', 'error');
            return;
        }

        // Parse command string into command + args
        let command = '';
        let args = [];
        if (commandRaw) {
            const parts = commandRaw.match(/(?:[^\s"]+|"[^"]*")+/g) || [];
            command = (parts[0] || '').replace(/^"|"$/g, '');
            args = parts.slice(1).map(a => a.replace(/^"|"$/g, ''));
        }

        // Get terminal size from main pane
        const entry = this.panes.get(vmName);
        const cols = entry?.term?.cols || 80;
        const rows = entry?.term?.rows || 24;

        this.send({
            type: 'create_session',
            agent_id: vmName,
            session_name: name,
            session_type: sessionType,
            command,
            args,
            working_dir: workingDir,
            cols,
            rows,
        });

        // Close modal
        document.getElementById('create-session-modal').classList.add('hidden');
        document.getElementById('create-session-form').reset();
        this.showToast(`Creating session "${name}"...`, 'info');
    }

    handleSessionCreated(msg) {
        this.showToast(`Session "${msg.session_name}" created`, 'success');

        // Refresh sessions blade if showing this agent
        if (msg.agent_id && this.selectedVmForSessions === msg.agent_id) {
            this.fetchSessionsForBlade(msg.agent_id);
        }

        // Auto-attach to interactive sessions
        if (msg.session_type === 'interactive' && msg.command_id) {
            const entry = this.panes.get(msg.agent_id);
            if (entry) {
                // Track as shell command so output routes to main terminal
                this.shellCommandIds.set(msg.agent_id, msg.command_id);
                entry.attachedSession = msg.command_id;
                entry.attachedSessionName = msg.session_name;

                // Clear and focus terminal
                if (entry.term) {
                    entry.term.clear();
                    entry.term.focus();
                }
            }
        }
    }

    formatDuration(seconds) {
        if (seconds < 60) return `${seconds}s`;
        if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
        return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
    }

    // =========================================================================
    // OAuth
    // =========================================================================

    detectOAuth(agentId, commandId, text) {
        for (const pattern of OAUTH_PATTERNS) {
            pattern.lastIndex = 0;
            const match = pattern.exec(text);
            if (match) {
                const url = (match[1] || match[0]).replace(/[.,;:!?'")\]}>]+$/, '');
                this.showOAuthModal({ agentId, commandId, url, message: text.trim() });
                return;
            }
        }
        for (const pattern of DEVICE_CODE_PATTERNS) {
            pattern.lastIndex = 0;
            const match = pattern.exec(text);
            if (match) {
                this.showToast(`Device code: ${match[1]}`, 'info');
            }
        }
    }

    showOAuthModal(prompt) {
        this.currentOAuthPrompt = prompt;
        document.getElementById('oauth-message').textContent =
            prompt.message.length > 200 ? prompt.message.substring(0, 200) + '...' : prompt.message;
        document.getElementById('oauth-link').href = prompt.url;
        document.getElementById('oauth-input').value = '';
        document.getElementById('oauth-modal').classList.remove('hidden');
        this.showToast(`Authorization required for ${prompt.agentId}`, 'info');
    }

    hideOAuthModal() {
        document.getElementById('oauth-modal').classList.add('hidden');
        this.currentOAuthPrompt = null;
    }

    submitOAuthInput() {
        const value = document.getElementById('oauth-input').value.trim();
        if (!value || !this.currentOAuthPrompt) return;
        const { agentId, commandId } = this.currentOAuthPrompt;
        this.send({
            type: 'send_input',
            agent_id: agentId,
            command_id: commandId || this.activeCommandIds.get(agentId),
            data: value + '\n',
        });
        this.hideOAuthModal();
    }

    // =========================================================================
    // Log Sidebar
    // =========================================================================

    setupLogSidebar() {
        const sidebar = document.getElementById('log-sidebar');
        const toggle = sidebar.querySelector('.sidebar-toggle');
        const tabs = sidebar.querySelectorAll('.tab-btn');
        const filterSelect = document.getElementById('event-filter');
        const clearBtn = document.getElementById('clear-events');
        const autoScrollCheckbox = document.getElementById('auto-scroll');

        // Toggle sidebar
        toggle.addEventListener('click', () => {
            sidebar.classList.toggle('collapsed');
        });

        // Tab switching
        tabs.forEach(tab => {
            tab.addEventListener('click', () => {
                tabs.forEach(t => t.classList.remove('active'));
                tab.classList.add('active');

                const panels = sidebar.querySelectorAll('.log-panel');
                panels.forEach(p => p.classList.remove('active'));

                const targetPanel = document.getElementById(`log-${tab.dataset.tab}`);
                if (targetPanel) targetPanel.classList.add('active');
            });
        });

        // Event filter
        filterSelect.addEventListener('change', (e) => {
            this.eventFilter = e.target.value;
            this.renderEventList();
        });

        // Clear events
        clearBtn.addEventListener('click', () => {
            this.logEvents = [];
            this.renderEventList();
        });

        // Auto-scroll toggle
        autoScrollCheckbox.addEventListener('change', (e) => {
            this.autoScroll = e.target.checked;
        });

        // Copy events to clipboard
        const copyBtn = document.getElementById('copy-events');
        copyBtn.addEventListener('click', () => this.copyEventsToClipboard());
    }

    copyEventsToClipboard() {
        // Get filtered events
        let events = this.logEvents;
        if (this.eventFilter !== 'all') {
            events = this.logEvents.filter(e => e.event_type === this.eventFilter);
        }

        // Format events as text
        const lines = events.map(event => {
            const time = this.formatEventTime(event.timestamp);
            const type = event.event_type || 'unknown';
            const source = event.agent_id || event.vm_name || 'unknown';

            let details = [];
            if (event.details) {
                if (event.details.hostname) details.push(`host=${event.details.hostname}`);
                if (event.details.ip_address && event.details.ip_address !== 'pending') {
                    details.push(`ip=${event.details.ip_address}`);
                }
                if (event.details.session_id) details.push(`session=${event.details.session_id.slice(0, 8)}`);
                if (event.details.reason) details.push(`reason=${event.details.reason}`);
            }

            const detailStr = details.length > 0 ? ` [${details.join(', ')}]` : '';
            return `${time}  ${type.padEnd(20)}  ${source}${detailStr}`;
        });

        const text = lines.join('\n');
        navigator.clipboard.writeText(text)
            .then(() => this.showToast('Events copied to clipboard', 'success'))
            .catch(() => this.showToast('Failed to copy', 'error'));
    }

    async fetchEvents() {
        try {
            const resp = await fetch('/api/v1/events');
            const data = await resp.json();

            // Only update if there are new events (prevents flickering)
            if (data.last_event_id && data.last_event_id === this.lastEventId) {
                return; // No new events
            }

            if (data.events) {
                this.lastEventId = data.last_event_id || 0;
                // Keep newest first, limit to maxLogEvents
                this.logEvents = data.events.slice(0, this.maxLogEvents);
                this.renderEventList();
            }
        } catch (e) {
            console.error('Failed to fetch events:', e);
        }
    }

    addEvent(event) {
        // Add new events at the beginning (newest first)
        this.logEvents.unshift(event);
        if (this.logEvents.length > this.maxLogEvents) {
            this.logEvents.pop();  // Remove oldest
        }
        this.renderEventList();
    }

    renderEventList() {
        const list = document.getElementById('event-list');
        const countEl = document.getElementById('event-count');

        // Filter events
        let filtered = this.logEvents;
        if (this.eventFilter !== 'all') {
            filtered = this.logEvents.filter(e => e.event_type === this.eventFilter);
        }

        countEl.textContent = `${filtered.length} events`;

        // Render (newest first)
        list.innerHTML = filtered.map(event => this.renderEventEntry(event)).join('');

        // Auto-scroll to top (newest events)
        if (this.autoScroll) {
            list.scrollTop = 0;
        }
    }

    renderEventEntry(event) {
        const eventType = event.event_type || 'unknown';
        // Handle vm.*, agent.*, and session.* event types
        const shortType = eventType.replace(/^(vm\.|agent\.|session\.)/, '');
        const isAgent = eventType.startsWith('agent.');
        const isSession = eventType.startsWith('session.');
        const cssClass = `event-${shortType.replace(/[._]/g, '-')}`;

        const time = this.formatEventTime(event.timestamp);
        const source = event.agent_id || event.vm_name || 'unknown';

        let details = '';
        if (event.details) {
            const parts = [];
            if (event.details.hostname) parts.push(`host: ${event.details.hostname}`);
            if (event.details.ip_address && event.details.ip_address !== 'pending') {
                parts.push(`ip: ${event.details.ip_address}`);
            }
            if (event.details.session_id) parts.push(`session: ${event.details.session_id.slice(0, 8)}`);
            if (event.details.command) parts.push(`cmd: ${event.details.command}`);
            if (event.details.reason) parts.push(event.details.reason);
            if (event.details.uptime_seconds) parts.push(`uptime: ${event.details.uptime_seconds}s`);
            // Session reconciliation details
            if (event.details.session_count !== undefined) parts.push(`sessions: ${event.details.session_count}`);
            if (event.details.keep_count !== undefined) parts.push(`kept: ${event.details.keep_count}`);
            if (event.details.kill_count !== undefined) parts.push(`killed: ${event.details.kill_count}`);
            if (event.details.failed_count !== undefined && event.details.failed_count > 0) {
                parts.push(`failed: ${event.details.failed_count}`);
            }
            details = parts.join(' | ');
        }

        // Determine type label prefix
        let typeLabel;
        if (isSession) {
            typeLabel = `session.${shortType}`;
        } else if (isAgent) {
            typeLabel = shortType;
        } else {
            typeLabel = `vm.${shortType}`;
        }

        // Special icons for session events
        let icon = '';
        if (isSession) {
            switch (shortType) {
                case 'query_sent': icon = '&#128269; '; break;      // magnifying glass
                case 'report_received': icon = '&#128203; '; break; // clipboard
                case 'reconcile_started': icon = '&#9881; '; break; // gear
                case 'reconcile_complete': icon = '&#10004; '; break; // checkmark
                case 'killed': icon = '&#10060; '; break;           // X
                case 'preserved': icon = '&#128994; '; break;       // green circle
                case 'reconcile_failed': icon = '&#9888; '; break;  // warning
            }
        }

        return `
            <div class="log-entry ${cssClass}">
                <div class="log-entry-header">
                    <span class="log-entry-type">${icon}${this.esc(typeLabel)}</span>
                    <span class="log-entry-time">${time}</span>
                </div>
                <div class="log-entry-vm">${this.esc(source)}</div>
                ${details ? `<div class="log-entry-details">${this.esc(details)}</div>` : ''}
            </div>
        `;
    }

    formatEventTime(timestamp) {
        if (!timestamp) return '--:--:--';
        const date = new Date(timestamp);
        return date.toLocaleTimeString('en-US', { hour12: false });
    }

    // =========================================================================
    // System Logs
    // =========================================================================

    handleSystemLog(msg) {
        const log = {
            level: msg.level || 'info',
            message: msg.message,
            target: msg.target || '',
            timestamp: msg.timestamp || new Date().toISOString(),
        };
        this.addSystemLog(log);
    }

    addSystemLog(log) {
        this.systemLogs.unshift(log);
        if (this.systemLogs.length > this.maxSystemLogs) {
            this.systemLogs.pop();
        }
        this.renderSystemLogs();
    }

    renderSystemLogs() {
        const list = document.getElementById('system-list');
        if (!list) return;

        if (this.systemLogs.length === 0) {
            list.innerHTML = '<div class="log-placeholder">No system logs</div>';
            return;
        }

        list.innerHTML = this.systemLogs.map(log => this.renderSystemLogEntry(log)).join('');

        if (this.autoScroll) {
            list.scrollTop = 0;
        }
    }

    renderSystemLogEntry(log) {
        const time = this.formatEventTime(log.timestamp);
        const level = log.level.toUpperCase();
        const levelClass = `log-level-${log.level}`;

        return `
            <div class="log-entry system-log ${levelClass}">
                <div class="log-entry-header">
                    <span class="log-entry-type">${this.esc(level)}</span>
                    <span class="log-entry-time">${time}</span>
                </div>
                ${log.target ? `<div class="log-entry-target">${this.esc(log.target)}</div>` : ''}
                <div class="log-entry-message">${this.esc(log.message)}</div>
            </div>
        `;
    }

    async fetchSystemLogs() {
        try {
            const resp = await fetch('/api/v1/logs');
            if (!resp.ok) {
                if (resp.status === 404) {
                    // API not implemented yet
                    return;
                }
                throw new Error(`HTTP ${resp.status}`);
            }
            const data = await resp.json();
            if (data.logs) {
                this.systemLogs = data.logs.slice(0, this.maxSystemLogs);
                this.renderSystemLogs();
            }
        } catch (e) {
            console.error('Failed to fetch system logs:', e);
        }
    }

    // =========================================================================
    // Global event listeners
    // =========================================================================

    setupGlobalListeners() {
        // OAuth modal
        document.querySelector('.modal-close').addEventListener('click', () => this.hideOAuthModal());
        document.querySelector('.modal-overlay').addEventListener('click', () => this.hideOAuthModal());
        document.getElementById('oauth-submit').addEventListener('click', () => this.submitOAuthInput());
        document.getElementById('oauth-input').addEventListener('keypress', (e) => {
            if (e.key === 'Enter') this.submitOAuthInput();
        });
        document.getElementById('copy-oauth-url').addEventListener('click', () => {
            navigator.clipboard.writeText(document.getElementById('oauth-link').href)
                .then(() => this.showToast('URL copied', 'success'));
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                this.hideOAuthModal();
                this.hideConfirmDialog();
            }
        });

        // Keepalive
        setInterval(() => {
            if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                this.send({ type: 'ping', timestamp: Date.now() });
            }
        }, 30000);

        // Periodic agent refresh
        setInterval(() => this.fetchAgents(), 10000);

        // Periodic event refresh (until WebSocket broadcast is implemented)
        setInterval(() => this.fetchEvents(), 5000);

        // Periodic VM list refresh
        setInterval(() => this.fetchVms(), 10000);

        // Periodic system log refresh
        setInterval(() => this.fetchSystemLogs(), 5000);
    }

    // =========================================================================
    // Utilities
    // =========================================================================

    stripAnsi(str) {
        // Remove ANSI escape sequences for clean thumbnail text
        return str.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, '')
                  .replace(/\x1b\][^\x07]*\x07/g, '')     // OSC sequences
                  .replace(/\x1b[()][0-9A-B]/g, '')        // charset selection
                  .replace(/\x1b\[[\?]?[0-9;]*[hlsr]/g, '') // mode set/reset
                  .replace(/\r\n/g, '\n')                  // normalize CRLF to LF
                  .replace(/\r/g, '');                     // remove standalone CR
    }

    updateSessionThumbs() {
        for (const [commandId, buf] of this.sessionBuffers) {
            if (!buf.dirty) continue;
            buf.dirty = false;

            // Find the session card's thumb-term element
            const el = document.querySelector(`.session-card[data-session-id="${commandId}"] .thumb-term`);
            if (!el) continue;

            // Split accumulated text on newlines, render last 6 lines
            const lines = buf.text.split('\n');
            const visibleLines = lines.slice(-6);
            el.textContent = visibleLines.join('\n');
        }
    }

    esc(text) {
        const d = document.createElement('div');
        d.textContent = text;
        return d.innerHTML;
    }

    showToast(message, type = 'info') {
        const container = document.getElementById('toast-container');
        const toast = document.createElement('div');
        toast.className = `toast ${type}`;
        toast.textContent = message;
        container.appendChild(toast);
        setTimeout(() => {
            toast.style.animation = 'slideIn 0.3s ease-out reverse';
            setTimeout(() => toast.remove(), 300);
        }, 4000);
    }
}

document.addEventListener('DOMContentLoaded', () => {
    window.dashboard = new AgenticDashboard();
});

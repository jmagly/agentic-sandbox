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

        this.init();
    }

    init() {
        this.setupGlobalListeners();
        this.connect();
        this.fetchAgents();
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
            case 'error':
                console.error('Server error:', msg.message);
                this.showToast(msg.message, 'error');
                break;
            default:
                console.log('Unknown message:', msg.type, msg);
        }
    }

    handleOutput(msg) {
        this.appendToPane(msg.agent_id, msg.stream, msg.data, msg.ts);
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

        // Focus the terminal
        const entry = this.panes.get(agent_id);
        if (entry && entry.term) {
            entry.term.focus();
            // Send initial resize to sync dimensions
            this.send({
                type: 'pty_resize',
                agent_id: agent_id,
                command_id: command_id,
                cols: entry.term.cols || 80,
                rows: entry.term.rows || 24,
            });
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
        const container = document.getElementById('pane-container');
        const pane = document.createElement('div');
        pane.className = 'agent-pane';
        pane.dataset.agentId = agent.id;

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
                    <button class="pane-shell-btn" title="Reconnect to tmux session">Reconnect</button>
                    <button class="pane-clear-btn">Clear</button>
                </div>
            </div>
            <div class="pane-output"></div>
        `;

        const outputEl = pane.querySelector('.pane-output');
        const clearBtn = pane.querySelector('.pane-clear-btn');
        const shellBtn = pane.querySelector('.pane-shell-btn');

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

        // Fit after DOM insertion
        requestAnimationFrame(() => { try { fitAddon.fit(); } catch (_) {} });

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

        clearBtn.addEventListener('click', () => { term.clear(); });

        // Shell button — reconnect to tmux session (kills old PTY, starts fresh attach)
        shellBtn.addEventListener('click', () => {
            term.clear();
            term.reset();
            this.startShell(agent.id);
        });

        this.panes.set(agent.id, { pane, output: outputEl, term, fitAddon, resizeObserver });

        // Auto-start shell for this agent
        this.startShell(agent.id);
    }

    updatePaneHeader(agent) {
        const entry = this.panes.get(agent.id);
        if (!entry) return;
        const dot = entry.pane.querySelector('.pane-status-dot');
        const statusClass = agent.status.toLowerCase().replace('agent_status_', '');
        dot.className = `pane-status-dot ${statusClass}`;
    }

    removePane(agentId) {
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
            if (e.key === 'Escape') this.hideOAuthModal();
        });

        // Keepalive
        setInterval(() => {
            if (this.ws && this.ws.readyState === WebSocket.OPEN) {
                this.send({ type: 'ping', timestamp: Date.now() });
            }
        }, 30000);

        // Periodic agent refresh
        setInterval(() => this.fetchAgents(), 10000);
    }

    // =========================================================================
    // Utilities
    // =========================================================================

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

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
        this.showToast('Connected to server', 'success');
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
            case 'command_started':
                this.activeCommandIds.set(msg.agent_id, msg.command_id);
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
                <div class="pane-controls">
                    <button class="pane-clear-btn">Clear</button>
                </div>
            </div>
            <div class="pane-output"></div>
            <div class="pane-command-bar">
                <input type="text" placeholder="$ type a command...">
                <button>Send</button>
            </div>
        `;

        const output = pane.querySelector('.pane-output');
        const input = pane.querySelector('.pane-command-bar input');
        const sendBtn = pane.querySelector('.pane-command-bar button');
        const clearBtn = pane.querySelector('.pane-clear-btn');

        const doSend = () => {
            const cmd = input.value.trim();
            if (!cmd) return;
            this.appendToPane(agent.id, 'log', `$ ${cmd}\n`, Date.now());
            this.send({
                type: 'send_command',
                agent_id: agent.id,
                command: 'bash',
                args: ['-c', cmd],
            });
            input.value = '';
            input.focus();
        };

        input.addEventListener('keypress', (e) => {
            if (e.key === 'Enter') doSend();
        });
        sendBtn.addEventListener('click', doSend);
        clearBtn.addEventListener('click', () => { output.innerHTML = ''; });

        container.appendChild(pane);
        this.panes.set(agent.id, { pane, output, input });
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

        const line = document.createElement('div');
        line.className = `output-line ${stream}`;

        const time = timestamp
            ? new Date(timestamp).toLocaleTimeString()
            : new Date().toLocaleTimeString();

        let content = this.esc(data);
        content = content.replace(
            /(https?:\/\/[^\s<>&"']+)/g,
            '<a href="$1" target="_blank" class="oauth-url" onclick="event.stopPropagation()">$1</a>'
        );

        line.innerHTML = `<span class="timestamp">[${time}]</span>${content}`;
        entry.output.appendChild(line);

        // Auto-scroll
        entry.output.scrollTop = entry.output.scrollHeight;

        // Buffer limit
        while (entry.output.children.length > 5000) {
            entry.output.removeChild(entry.output.firstChild);
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

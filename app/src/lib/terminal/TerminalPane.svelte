<script lang="ts">
	import { getAgentAuthToken } from '$lib/agentToken';
	import { workspaceHealth } from '$lib/workspaceHealth.svelte';
	import { onMount, onDestroy } from 'svelte';
	import { Terminal } from '@xterm/xterm';
	import { FitAddon } from '@xterm/addon-fit';
	import { WebglAddon } from '@xterm/addon-webgl';
	import { WebLinksAddon } from '@xterm/addon-web-links';
	import { SearchAddon } from '@xterm/addon-search';
	import { ClipboardAddon } from '@xterm/addon-clipboard';
	import { Unicode11Addon } from '@xterm/addon-unicode11';
	import { SerializeAddon } from '@xterm/addon-serialize';
	import '@xterm/xterm/css/xterm.css';
	import { isExternalHttpUrl } from '$lib/terminal/links';

	interface Props {
		agentBase: string;
		namespace: string;
		project: string;
		processId: string;
	}

	let { agentBase, namespace, project, processId }: Props = $props();

	let termEl: HTMLDivElement | undefined = $state();
	let terminal: Terminal | undefined;
	let resizeObserver: ResizeObserver | undefined;
	let mounted = false;
	let fit: FitAddon | undefined;
	let search: SearchAddon | undefined;
	let serialize: SerializeAddon | undefined;
	let socket: WebSocket | undefined;
	let agentToken: string | null = null;

	let connState = $state<'connecting' | 'connected' | 'reconnecting' | 'failed'>('connecting');
	let connectError = $state('');
	let searchOpen = $state(false);
	let searchTerm = $state('');

	let detectedLinks: string[] = $state([]);
	let linksOpen = $state(false);
	let linkScanBuf = '';
	const suppressedLinks = new Set<string>();
	const LINK_SCAN_MAX = 8192;
	const URL_RE = /https?:\/\/[^\s\x1b\x00-\x1f"'<>]+/g;
	const ANSI_RE = /\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))/g;

	let reconnectAttempts = 0;
	let reconnectTimer: ReturnType<typeof setTimeout> | undefined;

	const bufKey = `enzarb-term:${processId}`;
	let bufferCache: string | null = null;

	function scanForLinks(raw: string) {
		linkScanBuf = (linkScanBuf + raw).slice(-LINK_SCAN_MAX);
		const clean = linkScanBuf.replace(ANSI_RE, '');
		const found = clean.match(URL_RE) ?? [];
		for (const url of found) {
			const trimmed = url.replace(/[).,;:'"]+$/, '');
			if (!detectedLinks.includes(trimmed) && !suppressedLinks.has(trimmed)) {
				detectedLinks = [...detectedLinks, trimmed];
			}
		}
	}

	function saveBuffer() {
		if (!serialize) return;
		try {
			const data = serialize.serialize({ scrollback: 1000 });
			bufferCache = data;
			sessionStorage.setItem(bufKey, data);
		} catch {}
	}

	function loadBuffer(): string | null {
		if (bufferCache) return bufferCache;
		try { return sessionStorage.getItem(bufKey); } catch { return null; }
	}

	function runSearch(forward = true) {
		if (!searchTerm) return;
		const opts = { decorations: { matchOverviewRuler: '#888', activeMatchColorOverviewRuler: '#fff' } };
		if (forward) search?.findNext(searchTerm, opts);
		else search?.findPrevious(searchTerm, opts);
	}

	function send(data: string) {
		if (socket?.readyState === WebSocket.OPEN) socket.send(new TextEncoder().encode(data));
	}

	function sendResize() {
		if (socket?.readyState === WebSocket.OPEN && terminal) {
			socket.send(JSON.stringify({ rows: terminal.rows, cols: terminal.cols }));
		}
	}

	async function ensureToken(): Promise<string | null> {
		agentToken = await getAgentAuthToken(namespace, project);
		return agentToken;
	}

	async function openSocket() {
		if (!agentBase || !mounted) return;
		await workspaceHealth(agentBase).ensureHealthy();
		const token = await ensureToken();
		if (!token) {
			connState = 'failed';
			connectError = 'Session expired — please reload the page.';
			return;
		}

		if (socket) { socket.onclose = null; socket.onerror = null; socket.onmessage = null; socket.close(); socket = undefined; }

		terminal?.clear();
		const prev = loadBuffer();
		if (prev) {
			terminal?.write(prev);
			terminal?.write('\r\n\x1b[2m──────── reconnected ────────\x1b[0m\r\n');
		}
		connState = 'connecting';

		const wsUrl = `${agentBase.replace('https://', 'wss://').replace('http://', 'ws://')}/processes/${processId}/output`;
		socket = new WebSocket(wsUrl, ['bearer', token]);
		socket.binaryType = 'arraybuffer';
		socket.onopen = () => {
			reconnectAttempts = 0;
			connState = 'connected';
			connectError = '';
			suppressedLinks.clear();
			fit?.fit();
			sendResize();
		};
		socket.onerror = () => {
			connectError = 'WebSocket error — check that the workspace is running.';
		};
		socket.onclose = (e) => {
			socket = undefined;
			if (e.code === 1000 || e.code === 1001) return;
			if (e.code === 4004) {
				connState = 'failed';
				connectError = 'Process has exited.';
				return;
			}
			if (connState === 'failed' || !mounted) return;
			workspaceHealth(agentBase).suspect();
			connectError = `Disconnected (code ${e.code}) — reconnecting…`;
			connState = 'reconnecting';
			scheduleReconnect();
		};
		socket.onmessage = (e) => {
			const data = e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data;
			const text = typeof data === 'string' ? data : new TextDecoder().decode(data);
			terminal?.write(text);
			scanForLinks(text);
		};
	}

	const MAX_RECONNECT_ATTEMPTS = 6;
	function scheduleReconnect() {
		clearTimeout(reconnectTimer);
		reconnectAttempts++;
		if (reconnectAttempts > MAX_RECONNECT_ATTEMPTS) {
			connState = 'failed';
			connectError = 'Reconnection failed — the workspace may be unavailable.';
			return;
		}
		const delay = Math.min(1500 * 2 ** (reconnectAttempts - 1), 30_000);
		reconnectTimer = setTimeout(async () => {
			if (!mounted) return;
			if (socket?.readyState === WebSocket.OPEN || socket?.readyState === WebSocket.CONNECTING) return;
			await openSocket();
		}, delay);
	}

	function maybeReconnect() {
		if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return;
		if (socket?.readyState === WebSocket.OPEN || socket?.readyState === WebSocket.CONNECTING) return;
		reconnectAttempts = 0;
		connState = 'reconnecting';
		openSocket();
	}

	function onVisibility() {
		if (document.visibilityState === 'hidden') saveBuffer();
		else maybeReconnect();
	}

	onMount(async () => {
		mounted = true;
		terminal = new Terminal({
			theme: { background: '#0f0f11', foreground: '#e8e8ed' },
			fontFamily: 'JetBrains Mono, monospace',
			allowProposedApi: true,
			screenReaderMode: false
		});
		fit = new FitAddon();
		terminal.loadAddon(fit);
		const unicode11 = new Unicode11Addon();
		terminal.loadAddon(unicode11);
		terminal.unicode.activeVersion = '11';
		terminal.loadAddon(new ClipboardAddon());
		terminal.loadAddon(
			new WebLinksAddon((event, uri) => {
				if (!isExternalHttpUrl(uri)) return;
				window.open(uri, '_blank', 'noopener,noreferrer');
			})
		);
		search = new SearchAddon();
		terminal.loadAddon(search);
		serialize = new SerializeAddon();
		terminal.loadAddon(serialize);
		if (termEl) {
			terminal.open(termEl);
			fit.fit();
			try {
				const webgl = new WebglAddon();
				webgl.onContextLoss(() => { try { webgl.dispose(); } catch {} });
				terminal.loadAddon(webgl);
			} catch {}
		}
		terminal.onData((d: string) => send(d));
		resizeObserver = new ResizeObserver(() => { fit?.fit(); sendResize(); });
		if (termEl) resizeObserver.observe(termEl);
		await openSocket();
		document.addEventListener('visibilitychange', onVisibility);
		window.addEventListener('focus', maybeReconnect);
		window.addEventListener('online', maybeReconnect);
		window.addEventListener('beforeunload', saveBuffer);
	});

	onDestroy(() => {
		mounted = false;
		clearTimeout(reconnectTimer);
		resizeObserver?.disconnect();
		resizeObserver = undefined;
		document.removeEventListener('visibilitychange', onVisibility);
		window.removeEventListener('focus', maybeReconnect);
		window.removeEventListener('online', maybeReconnect);
		window.removeEventListener('beforeunload', saveBuffer);
		saveBuffer();
		if (socket) { socket.onclose = null; socket.onmessage = null; socket.onerror = null; socket.close(); socket = undefined; }
		const t = terminal;
		terminal = undefined;
		fit = undefined;
		search = undefined;
		serialize = undefined;
		try { t?.dispose(); } catch {}
	});
</script>

<div class="terminal-pane">
	<div class="pane-toolbar">
		<button class="tool-btn" title="Search output" onclick={() => { searchOpen = !searchOpen; if (!searchOpen) search?.clearDecorations(); }}>⌕</button>
		{#if detectedLinks.length}
			<button class="tool-btn link-btn" title="Links detected in output" onclick={() => linksOpen = !linksOpen}>
				🔗<span class="link-badge">{detectedLinks.length}</span>
			</button>
		{/if}
	</div>
	{#if searchOpen}
		<div class="search-bar">
			<!-- svelte-ignore a11y_autofocus -->
			<input
				class="search-input"
				placeholder="Search output…"
				autofocus
				bind:value={searchTerm}
				oninput={() => runSearch(true)}
				onkeydown={(e) => {
					if (e.key === 'Enter') { e.preventDefault(); runSearch(!e.shiftKey); }
					else if (e.key === 'Escape') { searchOpen = false; search?.clearDecorations(); }
				}}
			/>
			<button class="search-nav" onclick={() => runSearch(false)}>↑</button>
			<button class="search-nav" onclick={() => runSearch(true)}>↓</button>
			<button class="search-nav" onclick={() => { searchOpen = false; search?.clearDecorations(); }}>×</button>
		</div>
	{/if}
	{#if linksOpen && detectedLinks.length}
		<div class="links-panel">
			<div class="links-header">
				<span>Links</span>
				<button class="links-close" onclick={() => { for (const l of detectedLinks) suppressedLinks.add(l); detectedLinks = []; linkScanBuf = ''; linksOpen = false; }}>Clear ×</button>
			</div>
			{#each detectedLinks as url}
				<div class="link-row">
					<span class="link-url">{url}</span>
					<button class="link-action" onclick={() => navigator.clipboard.writeText(url)}>Copy</button>
					{#if isExternalHttpUrl(url)}
						<a class="link-action" href={url} target="_blank" rel="noopener noreferrer">Open</a>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
	{#if connState === 'reconnecting' || connState === 'failed'}
		<div class="connect-error" class:failed={connState === 'failed'}>
			<span>{connectError}</span>
			<button class="error-retry" onclick={() => { reconnectAttempts = 0; connState = 'reconnecting'; connectError = 'Reconnecting…'; openSocket(); }}>Retry</button>
			{#if connState === 'failed'}
				<button class="error-dismiss" onclick={() => { connState = 'connected'; connectError = ''; }}>×</button>
			{/if}
		</div>
	{/if}
	<div class="term-container" bind:this={termEl}></div>
</div>

<style>
	.terminal-pane { display: flex; flex-direction: column; height: 100%; overflow: hidden; background: #0f0f11; }
	.pane-toolbar { display: flex; align-items: center; gap: 0; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); }
	.tool-btn { width: 34px; height: 30px; border: none; border-right: 1px solid var(--color-border); background: none; color: var(--color-text-muted); font-size: 16px; cursor: pointer; position: relative; }
	.tool-btn:hover { background: var(--color-surface); color: var(--color-accent); }
	.link-btn { font-size: 13px; }
	.link-badge { position: absolute; top: 3px; right: 3px; background: var(--color-accent); color: #fff; border-radius: 8px; font-size: 9px; line-height: 1; padding: 1px 3px; pointer-events: none; }
	.search-bar { display: flex; align-items: center; gap: 0.25rem; padding: 0.35rem 0.5rem; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); }
	.search-input { flex: 1; min-width: 0; font-size: 13px; padding: 0.3rem 0.5rem; background: var(--color-surface); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; }
	.search-nav { flex-shrink: 0; width: 28px; height: 28px; border: 1px solid var(--color-border); border-radius: 4px; background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.search-nav:hover { color: var(--color-text); }
	.links-panel { border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); max-height: 160px; overflow-y: auto; }
	.links-header { display: flex; align-items: center; justify-content: space-between; padding: 0.3rem 0.5rem; font-size: 11px; color: var(--color-text-muted); border-bottom: 1px solid var(--color-border); }
	.links-close { background: none; border: none; cursor: pointer; font-size: 11px; color: var(--color-text-muted); padding: 0; }
	.links-close:hover { color: var(--color-danger); }
	.link-row { display: flex; align-items: center; gap: 0.4rem; padding: 0.3rem 0.5rem; border-bottom: 1px solid var(--color-border); }
	.link-row:last-child { border-bottom: none; }
	.link-url { flex: 1; min-width: 0; font-family: var(--font-mono); font-size: 11px; color: var(--color-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.link-action { flex-shrink: 0; font-size: 11px; padding: 0.15rem 0.4rem; border: 1px solid var(--color-border); border-radius: 3px; background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; text-decoration: none; }
	.link-action:hover { color: var(--color-accent); border-color: var(--color-accent); }
	.connect-error { display: flex; align-items: center; gap: 0.5rem; padding: 0.35rem 0.75rem; background: #3d2000; color: #f5a623; font-size: 12px; border-bottom: 1px solid #7a4500; }
	.connect-error span { flex: 1; }
	.connect-error.failed { background: #3d0000; color: #f56262; border-color: #7a0000; }
	.error-retry { background: none; border: 1px solid currentColor; border-radius: 3px; color: inherit; cursor: pointer; font-size: 11px; padding: 0.1rem 0.5rem; flex-shrink: 0; opacity: 0.85; }
	.error-retry:hover { opacity: 1; }
	.error-dismiss { background: none; border: none; color: inherit; cursor: pointer; font-size: 16px; line-height: 1; padding: 0; flex-shrink: 0; opacity: 0.7; }
	.error-dismiss:hover { opacity: 1; }
	.term-container { background: #0f0f11; overflow: hidden; flex: 1; min-height: 0; }
</style>

<script lang="ts">
	import { getAgentToken, getProject } from '$lib/remote/projects.remote';
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
	import VirtualKeyboard from '$lib/terminal/VirtualKeyboard.svelte';
	import { layouts } from '$lib/terminal/keyboard';
	import { isExternalHttpUrl } from '$lib/terminal/links';

	// Each project gets its own agent route (e.g. `/agent/<slug>`); the path is
	// published in the Project's status by the operator.
	let agentBase = $state('');

	let termEl: HTMLDivElement | undefined = $state();
	let newDialog: HTMLDialogElement | undefined = $state();
	let terminal: Terminal | undefined;
	let resizeObserver: ResizeObserver | undefined;
	let mounted = false;
	let ws: WebSocket | null = null;
	let processes: any[] = $state([]);
	let selectedPid: string | null = $state(null);
	let newCmd = $state('');
	let newName = $state('');
	let newKind: 'one-shot' | 'persistent' = $state('one-shot');
	let createErr = $state('');
	let agentToken: string | null = $state(null);
	let connectError = $state('');

	// On touch devices we suppress the OS keyboard and drive input from our own
	// on-screen keyboard instead (Ctrl/Alt/Fn aren't reachable otherwise).
	let isTouch = $state(false);
	let fit: FitAddon | undefined;
	let search: SearchAddon | undefined;
	let serialize: SerializeAddon | undefined;
	// The process currently rendered in the buffer; differs from selectedPid only
	// mid-switch. Used to decide when to clear vs. preserve scrollback.
	let displayedPid: string | null = null;
	// Per-process serialized scrollback, mirrored to sessionStorage so a refresh
	// can restore it (the agent's tmux attach only redraws the live screen).
	const bufferCache: Record<string, string> = {};
	let searchOpen = $state(false);
	let searchTerm = $state('');

	// Detected URLs from terminal output — surfaced so the user can copy/open
	// them even when they wrap across multiple terminal lines.
	let detectedLinks: string[] = $state([]);
	let linksOpen = $state(false);
	// Rolling text window used for URL detection across chunk boundaries.
	let linkScanBuf = '';
	// Links cleared by the user — suppressed until the next reconnect.
	const suppressedLinks = new Set<string>();
	const LINK_SCAN_MAX = 8192;
	const URL_RE = /https?:\/\/[^\s\x1b\x00-\x1f"'<>]+/g;
	// Strip ANSI/VT escape sequences before scanning.
	const ANSI_RE = /\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\))/g;

	function scanForLinks(raw: string) {
		linkScanBuf = (linkScanBuf + raw).slice(-LINK_SCAN_MAX);
		const clean = linkScanBuf.replace(ANSI_RE, '');
		const found = clean.match(URL_RE) ?? [];
		for (const url of found) {
			// Trim trailing punctuation that's likely not part of the URL.
			const trimmed = url.replace(/[).,;:'"]+$/, '');
			if (!detectedLinks.includes(trimmed) && !suppressedLinks.has(trimmed)) {
				detectedLinks = [...detectedLinks, trimmed];
			}
		}
	}

	const bufKey = (pid: string) => `enzarb-term:${pid}`;
	function saveBuffer(pid: string | null) {
		if (!pid || !serialize) return;
		try {
			const data = serialize.serialize({ scrollback: 1000 });
			bufferCache[pid] = data;
			sessionStorage.setItem(bufKey(pid), data);
		} catch {
			/* serialize/sessionStorage may fail (quota); non-fatal */
		}
	}
	function loadBuffer(pid: string): string | null {
		if (bufferCache[pid]) return bufferCache[pid];
		try {
			return sessionStorage.getItem(bufKey(pid));
		} catch {
			return null;
		}
	}

	function runSearch(forward = true) {
		if (!searchTerm) return;
		const opts = { decorations: { matchOverviewRuler: '#888', activeMatchColorOverviewRuler: '#fff' } };
		if (forward) search?.findNext(searchTerm, opts);
		else search?.findPrevious(searchTerm, opts);
	}

	// Input is sent as binary frames; the agent reserves text frames for control
	// messages (terminal resize). Both xterm input and the virtual keyboard
	// funnel through here.
	function send(data: string) {
		if (ws?.readyState === WebSocket.OPEN) ws.send(new TextEncoder().encode(data));
	}

	// Tell the agent the PTY dimensions so output wraps/clears correctly.
	function sendResize() {
		if (ws?.readyState === WebSocket.OPEN && terminal) {
			ws.send(JSON.stringify({ rows: terminal.rows, cols: terminal.cols }));
		}
	}

	async function loadProcesses() {
		if (!agentToken || !agentBase) return;
		const res = await fetch(`${agentBase}/processes`, { headers: { Authorization: `Bearer ${agentToken}` } });
		if (res.ok) processes = await res.json();
	}

	function openNewDialog() {
		newCmd = ''; newName = ''; newKind = 'one-shot'; createErr = '';
		newDialog?.showModal();
	}

	async function createProcess() {
		createErr = '';
		if (!agentToken || !agentBase) { createErr = 'Not connected to the workspace agent.'; return; }
		if (!newCmd.trim()) { createErr = 'Enter a command.'; return; }
		let res: Response;
		try {
			res = await fetch(`${agentBase}/processes`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${agentToken}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ name: newName || newCmd, command: newCmd, kind: newKind })
			});
		} catch {
			createErr = 'Could not reach the workspace agent.';
			return;
		}
		if (!res.ok) {
			createErr = `Failed to start process (${res.status}). ${(await res.text().catch(() => '')).slice(0, 200)}`.trim();
			return;
		}
		const p = await res.json();
		newCmd = ''; newName = '';
		newDialog?.close();
		await loadProcesses();
		attachToProcess(p.id);
	}

	async function killProcess(id: string) {
		if (!agentToken) return;
		await fetch(`${agentBase}/processes/${id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${agentToken}` } });
		await loadProcesses();
		delete bufferCache[id];
		try { sessionStorage.removeItem(bufKey(id)); } catch { /* ignore */ }
		if (selectedPid === id) {
			ws?.close(); ws = null; selectedPid = null; displayedPid = null;
			try { sessionStorage.removeItem(SELECTED_KEY); } catch { /* ignore */ }
		}
	}

	const SELECTED_KEY = 'enzarb-term:selected';

	// Selecting a tab sets the desired process and (re)connects. selectedPid is
	// kept across drops so we know what to reconnect to. Persisted to
	// sessionStorage so a full page reload (mobile tab kill/restore) returns to
	// the same process rather than blindly attaching to processes[0].
	function attachToProcess(id: string) {
		selectedPid = id;
		try { sessionStorage.setItem(SELECTED_KEY, id); } catch { /* ignore */ }
		connect();
	}

	// Agent tokens are short-lived; re-mint on (re)connect so a socket opened after
	// a long mobile background isn't rejected for an expired token.
	async function ensureToken() {
		try { agentToken = await getAgentToken(); } catch { /* keep any existing token */ }
		return agentToken;
	}

	async function connect() {
		if (!agentBase || !selectedPid) return;
		await ensureToken();
		if (!agentToken) return;
		ws?.close();
		// Only reset the buffer when switching to a different process: stash the
		// outgoing one's scrollback and restore the incoming one's. A same-process
		// reconnect (mobile resume) keeps the existing buffer; tmux redraws the
		// live screen on top.
		if (displayedPid !== selectedPid) {
			if (displayedPid) saveBuffer(displayedPid);
			terminal?.clear();
			const prev = loadBuffer(selectedPid);
			if (prev) {
				terminal?.write(prev);
				terminal?.write('\r\n\x1b[2m──────── reconnected ────────\x1b[0m\r\n');
			}
			displayedPid = selectedPid;
		}
		const wsUrl = `${agentBase.replace('https://', 'wss://').replace('http://', 'ws://')}/processes/${selectedPid}/output`;
		const sock = new WebSocket(`${wsUrl}?token=${encodeURIComponent(agentToken)}`);
		ws = sock;
		// The agent streams output as binary frames; default binaryType is "blob",
		// which TextDecoder can't decode. Use arraybuffer so we can render it.
		sock.binaryType = 'arraybuffer';
		// On connect, refit and report the size so the PTY matches the viewport.
		sock.onopen = () => { connectError = ''; suppressedLinks.clear(); fit?.fit(); sendResize(); };
		sock.onerror = () => { connectError = 'WebSocket error — check that the workspace is running and reachable.'; };
		sock.onclose = (e) => {
			if (e.code !== 1000 && e.code !== 1001) {
				connectError = `Connection closed (code ${e.code})${e.reason ? ': ' + e.reason : ''}.`;
			}
		};
		sock.onmessage = (e) => {
			const data = e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data;
			const text = typeof data === 'string' ? data : new TextDecoder().decode(data);
			terminal?.write(text);
			scanForLinks(text);
		};
		// Leave selectedPid set so maybeReconnect can re-establish the socket.
	}

	// Mobile browsers close the socket when the tab is backgrounded. Reconnect
	// when the page becomes visible/focused again (or the network returns) if we
	// have a selected process and the socket isn't already open/connecting.
	function maybeReconnect() {
		if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return;
		if (!selectedPid) return;
		const st = ws?.readyState;
		if (st === WebSocket.OPEN || st === WebSocket.CONNECTING) return;
		connect();
	}

	// Snapshot scrollback when the tab is hidden (mobile may discard it), and
	// reconnect when it becomes visible again.
	function onVisibility() {
		if (document.visibilityState === 'hidden') saveBuffer(displayedPid);
		else maybeReconnect();
	}

	onMount(async () => {
		mounted = true;
		try { agentToken = await getAgentToken(); } catch {}
		if (!mounted) return;
		try {
			const project = await getProject();
			const path = project?.status?.agentPath;
			if (path) agentBase = `https://enzarb.dev${path}`;
		} catch {}
		if (!mounted) return;
		isTouch = window.matchMedia('(pointer: coarse)').matches;
		terminal = new Terminal({ theme: { background: '#0f0f11', foreground: '#e8e8ed' }, fontFamily: 'JetBrains Mono, monospace', allowProposedApi: true, screenReaderMode: false });
		fit = new FitAddon();
		terminal.loadAddon(fit);
		// Correct width for wide chars/emoji.
		const unicode11 = new Unicode11Addon();
		terminal.loadAddon(unicode11);
		terminal.unicode.activeVersion = '11';
		// OSC 52 clipboard so workspace programs can copy to the browser clipboard.
		terminal.loadAddon(new ClipboardAddon());
		// Clickable links, but only safe external http(s) targets — never the
		// user's loopback/private network (see isExternalHttpUrl).
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
			// GPU renderer for smooth output; fall back to the DOM renderer if the
			// WebGL context is lost (common after mobile backgrounding).
			try {
				const webgl = new WebglAddon();
				webgl.onContextLoss(() => webgl.dispose());
				terminal.loadAddon(webgl);
			} catch {
				/* no WebGL available; DOM renderer remains */
			}
		}
		terminal.onData((d: string) => send(d));
		// Keep xterm focusable (paste, cursor) but stop the OS keyboard from
		// covering the screen — our virtual keyboard takes over on touch.
		if (isTouch) {
			const ta = termEl?.querySelector('.xterm-helper-textarea');
			if (ta) ta.setAttribute('inputmode', 'none');
		}
		resizeObserver = new ResizeObserver(() => { fit?.fit(); sendResize(); });
		if (termEl) resizeObserver.observe(termEl);
		if (!mounted) return;
		await loadProcesses();
		// Auto-attach: prefer the last-used process (survives full page reload on
		// mobile) and fall back to the first available one.
		if (!mounted) return;
		if (!selectedPid && processes.length) {
			let savedId: string | null = null;
			try { savedId = sessionStorage.getItem(SELECTED_KEY); } catch { /* ignore */ }
			const target = processes.find((p: any) => p.id === savedId) ?? processes[0];
			attachToProcess(target.id);
		}
		// Reconnect when returning to a backgrounded mobile tab or after a network drop.
		document.addEventListener('visibilitychange', onVisibility);
		window.addEventListener('focus', maybeReconnect);
		window.addEventListener('online', maybeReconnect);
		// Persist scrollback so a full page refresh can restore it.
		window.addEventListener('beforeunload', persist);
	});

	function persist() { saveBuffer(displayedPid); }

	onDestroy(() => {
		mounted = false;
		resizeObserver?.disconnect();
		resizeObserver = undefined;
		document.removeEventListener('visibilitychange', onVisibility);
		window.removeEventListener('focus', maybeReconnect);
		window.removeEventListener('online', maybeReconnect);
		window.removeEventListener('beforeunload', persist);
		persist();
		ws?.close();
		ws = null;
		terminal?.dispose();
		terminal = undefined;
		fit = undefined;
		search = undefined;
		serialize = undefined;
	});
</script>

<div class="terminal-page" class:touch={isTouch}>
	<div class="tab-bar">
		<div class="tabs">
			{#each processes as p}
				<div
					class="tab {selectedPid === p.id ? 'active' : ''}"
					role="button" tabindex="0"
					title={p.name}
					onclick={() => attachToProcess(p.id)}
					onkeydown={(e) => e.key === 'Enter' && attachToProcess(p.id)}
				>
					<span class="status-dot {p.status}"></span>
					<span class="tab-name">{p.name}</span>
					<button class="tab-close" title="Stop" onclick={(e) => { e.stopPropagation(); killProcess(p.id); }}>×</button>
				</div>
			{:else}
				<p class="muted">No processes — start one with +</p>
			{/each}
		</div>
		<button class="new-btn" title="Search output" aria-label="Search output" onclick={() => { searchOpen = !searchOpen; if (!searchOpen) search?.clearDecorations(); }}>⌕</button>
		{#if detectedLinks.length}
			<button class="new-btn link-btn" title="Links detected in output" aria-label="Show links" onclick={() => linksOpen = !linksOpen}>
				🔗<span class="link-badge">{detectedLinks.length}</span>
			</button>
		{/if}
		<button class="new-btn" title="New process" aria-label="New process" onclick={openNewDialog}>+</button>
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
			<button class="search-nav" title="Previous" aria-label="Previous match" onclick={() => runSearch(false)}>↑</button>
			<button class="search-nav" title="Next" aria-label="Next match" onclick={() => runSearch(true)}>↓</button>
			<button class="search-nav" title="Close" aria-label="Close search" onclick={() => { searchOpen = false; search?.clearDecorations(); }}>×</button>
		</div>
	{/if}
	{#if linksOpen && detectedLinks.length}
		<div class="links-panel">
			<div class="links-header">
				<span>Links detected in output</span>
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
	{#if connectError}
		<div class="connect-error">
			<span>{connectError}</span>
			<button class="error-dismiss" onclick={() => connectError = ''}>×</button>
		</div>
	{/if}
	<div class="term-container" bind:this={termEl}></div>
	{#if isTouch}
		<VirtualKeyboard {send} layout={layouts[0]} />
	{/if}
</div>

<dialog bind:this={newDialog} class="new-dialog" onclose={() => {}}>
	<form method="dialog" class="new-form" onsubmit={(e) => { e.preventDefault(); createProcess(); }}>
		<h3>New process</h3>
		<label>
			<span>Name <span class="muted">(optional)</span></span>
			<input bind:value={newName} placeholder="My task" />
		</label>
		<label>
			<span>Command</span>
			<!-- svelte-ignore a11y_autofocus -->
			<input bind:value={newCmd} placeholder="npm run dev" autofocus required />
		</label>
		<label>
			<span>Kind</span>
			<select bind:value={newKind}>
				<option value="one-shot">One-shot</option>
				<option value="persistent">Persistent</option>
			</select>
		</label>
		{#if createErr}<p class="create-err">{createErr}</p>{/if}
		<div class="dialog-actions">
			<button type="button" class="btn" onclick={() => newDialog?.close()}>Cancel</button>
			<button type="submit" class="btn btn-primary">Run</button>
		</div>
	</form>
</dialog>

<style>
	.terminal-page { display: flex; flex-direction: column; gap: 0; height: calc(100vh - 200px); min-height: 400px; margin-top: -1.5rem; }
	@media (max-width: 640px) {
		.terminal-page { height: calc(100vh - 120px); }
	}

	@media (max-width: 768px) {
		.terminal-page { height: calc(100dvh - 180px); }
	}

	.tab-bar { display: flex; align-items: stretch; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); }
	.tabs { flex: 1; display: flex; align-items: stretch; overflow-x: auto; }
	.tab { display: flex; align-items: center; gap: 0.4rem; padding: 0 0.5rem 0 0.75rem; max-width: 200px; border: none; border-right: 1px solid var(--color-border); background: none; color: var(--color-text-muted); font-size: 12px; cursor: pointer; white-space: nowrap; }
	.tab:hover { background: var(--color-surface); color: var(--color-text); }
	.tab.active { background: #0f0f11; color: var(--color-text); box-shadow: inset 0 2px 0 var(--color-accent); }
	.tab-name { overflow: hidden; text-overflow: ellipsis; font-family: var(--font-mono); }
	.status-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--color-text-muted); flex-shrink: 0; }
	.status-dot.running { background: #3fb950; }
	.status-dot.exited { background: var(--color-text-muted); }
	.status-dot.failed { background: var(--color-danger); }
	.tab-close { background: none; border: none; color: var(--color-text-muted); cursor: pointer; font-size: 15px; line-height: 1; padding: 0 0.125rem; border-radius: 3px; }
	.tab-close:hover { color: var(--color-danger); background: var(--color-surface-2); }
	.new-btn { flex-shrink: 0; width: 38px; border: none; border-left: 1px solid var(--color-border); background: none; color: var(--color-text-muted); font-size: 20px; line-height: 1; cursor: pointer; }
	.new-btn:hover { background: var(--color-surface); color: var(--color-accent); }

	.search-bar { display: flex; align-items: center; gap: 0.25rem; padding: 0.35rem 0.5rem; border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); }
	.search-input { flex: 1; min-width: 0; font-size: 13px; padding: 0.3rem 0.5rem; background: var(--color-surface); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; }
	.search-nav { flex-shrink: 0; width: 28px; height: 28px; border: 1px solid var(--color-border); border-radius: 4px; background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.search-nav:hover { color: var(--color-text); }

	.link-btn { position: relative; font-size: 14px; }
	.link-badge { position: absolute; top: 4px; right: 4px; background: var(--color-accent); color: #fff; border-radius: 8px; font-size: 9px; line-height: 1; padding: 1px 3px; pointer-events: none; }

	.links-panel { border-bottom: 1px solid var(--color-border); background: var(--color-surface-2); max-height: 160px; overflow-y: auto; }
	.links-header { display: flex; align-items: center; justify-content: space-between; padding: 0.3rem 0.5rem; font-size: 11px; color: var(--color-text-muted); border-bottom: 1px solid var(--color-border); }
	.links-close { background: none; border: none; cursor: pointer; font-size: 11px; color: var(--color-text-muted); padding: 0; }
	.links-close:hover { color: var(--color-danger); }
	.link-row { display: flex; align-items: center; gap: 0.4rem; padding: 0.3rem 0.5rem; border-bottom: 1px solid var(--color-border); }
	.link-row:last-child { border-bottom: none; }
	.link-url { flex: 1; min-width: 0; font-family: var(--font-mono); font-size: 11px; color: var(--color-text); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.link-action { flex-shrink: 0; font-size: 11px; padding: 0.15rem 0.4rem; border: 1px solid var(--color-border); border-radius: 3px; background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; text-decoration: none; }
	.link-action:hover { color: var(--color-accent); border-color: var(--color-accent); }

	.connect-error { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; padding: 0.35rem 0.75rem; background: #3d2000; color: #f5a623; font-size: 12px; border-bottom: 1px solid #7a4500; }
	.error-dismiss { background: none; border: none; color: #f5a623; cursor: pointer; font-size: 16px; line-height: 1; padding: 0; flex-shrink: 0; }
	.error-dismiss:hover { color: #fff; }
	.term-container { background: #0f0f11; overflow: hidden; flex: 1; min-height: 0; }
	.muted { color: var(--color-text-muted); font-size: 12px; padding: 0 0.75rem; align-self: center; }

	.new-dialog { border: 1px solid var(--color-border); border-radius: 8px; background: var(--color-surface); color: var(--color-text); padding: 0; min-width: 320px; }
	.new-dialog::backdrop { background: rgba(0, 0, 0, 0.5); }
	.new-form { display: flex; flex-direction: column; gap: 0.75rem; padding: 1.25rem; }
	.new-form h3 { margin: 0; font-size: 15px; }
	.new-form label { display: flex; flex-direction: column; gap: 0.25rem; font-size: 12px; font-weight: 500; }
	.new-form input, .new-form select { font-size: 13px; }
	.dialog-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.25rem; }
	.create-err { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0 0; }
</style>

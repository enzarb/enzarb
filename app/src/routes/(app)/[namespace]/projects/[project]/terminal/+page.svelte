<script lang="ts">
	import { getProject } from '$lib/remote/projects.remote';
	import { getAgentAuthToken } from '$lib/agentToken';
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
	// One persistent socket per process; switching tabs just changes which one
	// feeds the terminal rather than closing and reopening a connection.
	const sockets = new Map<string, WebSocket>();
	let processes: any[] = $state([]);
	let selectedPid: string | null = $state(null);
	let newCmd = $state('');
	let newName = $state('');
	let newKind: 'one-shot' | 'persistent' = $state('one-shot');
	let newCwd = $state('');
	let createErr = $state('');
	let workspacePaths: { home_dir: string; project_dir: string | null } | null = $state(null);
	let agentToken: string | null = $state(null);
	// 'connecting' | 'connected' | 'reconnecting' | 'failed'
	let connState = $state<'connecting' | 'connected' | 'reconnecting' | 'failed'>('connecting');
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
	// can restore it without waiting for the agent to replay the buffer.
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
		const sock = selectedPid ? sockets.get(selectedPid) : undefined;
		if (sock?.readyState === WebSocket.OPEN) sock.send(new TextEncoder().encode(data));
	}

	// Tell the agent the PTY dimensions so output wraps/clears correctly.
	function sendResize() {
		const sock = selectedPid ? sockets.get(selectedPid) : undefined;
		if (sock?.readyState === WebSocket.OPEN && terminal) {
			sock.send(JSON.stringify({ rows: terminal.rows, cols: terminal.cols }));
		}
	}

	async function loadProcesses() {
		if (!agentBase) return;
		const token = await ensureToken();
		if (!token) return;
		const res = await fetch(`${agentBase}/processes`, { headers: { Authorization: `Bearer ${token}` } });
		if (res.ok) processes = await res.json();
	}

	async function openNewDialog() {
		newCmd = ''; newName = ''; newKind = 'one-shot'; newCwd = ''; createErr = '';
		if (!workspacePaths && agentBase) {
			const token = await ensureToken();
			if (token) {
				try {
					const res = await fetch(`${agentBase}/status`, { headers: { Authorization: `Bearer ${token}` } });
					if (res.ok) workspacePaths = await res.json();
				} catch { /* non-fatal */ }
			}
		}
		newDialog?.showModal();
	}

	async function createProcess() {
		createErr = '';
		if (!agentBase) { createErr = 'Not connected to the workspace agent.'; return; }
		const token = await ensureToken();
		if (!token) { createErr = 'Not connected to the workspace agent.'; return; }
		const parts = newCmd.trim().split(/\s+/);
		if (!parts.length || !parts[0]) { createErr = 'Enter a command.'; return; }
		const [command, ...args] = parts;
		let res: Response;
		try {
			res = await fetch(`${agentBase}/processes`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ name: newName || newCmd.trim(), command, args, kind: newKind, ...(newCwd ? { cwd: newCwd } : {}) })
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
		const token = await ensureToken();
		if (!token) return;
		await fetch(`${agentBase}/processes/${id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${token}` } });
		const sock = sockets.get(id);
		if (sock) { sock.onclose = null; sock.close(); sockets.delete(id); }
		reconnectAttempts.delete(id);
		await loadProcesses();
		delete bufferCache[id];
		try { sessionStorage.removeItem(bufKey(id)); } catch { /* ignore */ }
		if (selectedPid === id) {
			selectedPid = null; displayedPid = null;
			try { sessionStorage.removeItem(SELECTED_KEY); } catch { /* ignore */ }
		}
	}

	const SELECTED_KEY = 'enzarb-term:selected';

	// Selecting a tab switches which process feeds the terminal. If a socket for
	// that process already exists and is open we just swap output; otherwise we
	// open a new one. The previous process's socket stays alive so switching back
	// is instant and doesn't trigger a spurious reconnect error.
	function attachToProcess(id: string) {
		if (displayedPid && displayedPid !== id) saveBuffer(displayedPid);
		selectedPid = id;
		try { sessionStorage.setItem(SELECTED_KEY, id); } catch { /* ignore */ }

		const existing = sockets.get(id);
		if (existing && (existing.readyState === WebSocket.OPEN || existing.readyState === WebSocket.CONNECTING)) {
			// Socket already live — just switch the terminal display.
			displayedPid = id;
			terminal?.clear();
			const prev = loadBuffer(id);
			if (prev) terminal?.write(prev);
			connState = existing.readyState === WebSocket.OPEN ? 'connected' : 'connecting';
			connectError = '';
			fit?.fit();
			sendResize();
		} else {
			openSocket(id);
		}
	}

	async function ensureToken(): Promise<string | null> {
		agentToken = await getAgentAuthToken();
		return agentToken;
	}

	async function openSocket(pid: string) {
		if (!agentBase) return;
		await ensureToken();
		if (!agentToken) {
			if (pid === selectedPid) {
				connState = 'failed';
				connectError = 'Session expired — please reload the page to sign in again.';
			}
			return;
		}

		// Close any stale socket for this pid before opening a fresh one.
		const stale = sockets.get(pid);
		if (stale) { stale.onclose = null; stale.onerror = null; stale.onmessage = null; stale.close(); sockets.delete(pid); }

		if (pid === selectedPid) {
			terminal?.clear();
			const prev = loadBuffer(pid);
			if (prev) {
				terminal?.write(prev);
				terminal?.write('\r\n\x1b[2m──────── reconnected ────────\x1b[0m\r\n');
			}
			displayedPid = pid;
			connState = 'connecting';
		}

		const wsUrl = `${agentBase.replace('https://', 'wss://').replace('http://', 'ws://')}/processes/${pid}/output`;
		// Carry the JWT in the Sec-WebSocket-Protocol header (via the subprotocol
		// list) rather than the URL, so it never lands in access/proxy logs. The
		// agent reads the `bearer, <jwt>` pair and echoes back only `bearer`.
		const sock = new WebSocket(wsUrl, ['bearer', agentToken]);
		sockets.set(pid, sock);
		// The agent streams output as binary frames; default binaryType is "blob",
		// which TextDecoder can't decode. Use arraybuffer so we can render it.
		sock.binaryType = 'arraybuffer';
		sock.onopen = () => {
			reconnectAttempts.delete(pid);
			if (pid === selectedPid) {
				connState = 'connected';
				connectError = '';
				suppressedLinks.clear();
				fit?.fit();
				sendResize();
			}
		};
		sock.onerror = () => {
			if (pid === selectedPid) {
				connectError = 'WebSocket error — check that the workspace is running and reachable.';
			}
		};
		sock.onclose = (e) => {
			sockets.delete(pid);
			// 1000/1001 = clean close (process killed or navigated away); ignore those.
			if (e.code === 1000 || e.code === 1001) return;
			if (pid === selectedPid) {
				// 4004 = the agent has confirmed the process is gone for good
				// (exited or never existed) — retrying would just loop forever
				// against a dead process, so stop here.
				if (e.code === 4004) {
					reconnectAttempts.delete(pid);
					connState = 'failed';
					connectError = 'Process has exited.';
					return;
				}
				if (connState === 'failed') return;
				const reason = e.reason ? `: ${e.reason}` : '';
				connectError = `Disconnected (code ${e.code}${reason}) — attempting to reconnect…`;
				connState = 'reconnecting';
				scheduleReconnect(pid);
			}
		};
		sock.onmessage = (e) => {
			// Only write to terminal if this is the process currently on display.
			if (pid !== displayedPid) return;
			const data = e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data;
			const text = typeof data === 'string' ? data : new TextDecoder().decode(data);
			terminal?.write(text);
			scanForLinks(text);
		};
	}

	// Auto-reconnect with exponential backoff, capped at MAX_RECONNECT_ATTEMPTS.
	// Each attempt is counted here (not via a separate readyState poll), so a
	// process that keeps refusing the connection can't loop forever — it
	// eventually lands in 'failed' and waits for the user to click Retry.
	const MAX_RECONNECT_ATTEMPTS = 6;
	const reconnectAttempts = new Map<string, number>();
	let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
	function scheduleReconnect(pid: string) {
		clearTimeout(reconnectTimer);
		const attempt = (reconnectAttempts.get(pid) ?? 0) + 1;
		reconnectAttempts.set(pid, attempt);
		if (attempt > MAX_RECONNECT_ATTEMPTS) {
			if (pid === selectedPid) {
				connState = 'failed';
				connectError = 'Reconnection failed — the workspace may be unavailable.';
			}
			return;
		}
		const delay = Math.min(1500 * 2 ** (attempt - 1), 30_000);
		reconnectTimer = setTimeout(async () => {
			if (!mounted || pid !== selectedPid) return;
			const st = sockets.get(pid)?.readyState;
			if (st === WebSocket.OPEN || st === WebSocket.CONNECTING) return;
			await openSocket(pid);
		}, delay);
	}

	// Mobile browsers close sockets when the tab is backgrounded. Reconnect all
	// processes (or at least the selected one) when the page becomes visible again.
	function maybeReconnect() {
		if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return;
		if (!selectedPid) return;
		const st = sockets.get(selectedPid)?.readyState;
		if (st === WebSocket.OPEN || st === WebSocket.CONNECTING) return;
		reconnectAttempts.delete(selectedPid);
		connState = 'reconnecting';
		openSocket(selectedPid);
	}

	// Snapshot scrollback when the tab is hidden (mobile may discard it), and
	// reconnect when it becomes visible again.
	function onVisibility() {
		if (document.visibilityState === 'hidden') saveBuffer(displayedPid);
		else maybeReconnect();
	}

	onMount(async () => {
		mounted = true;
		await ensureToken();
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
			// WebGL context is lost (common after mobile backgrounding/tab switches).
			try {
				const webgl = new WebglAddon();
				// Terminal.dispose() (see onDestroy) also disposes every loaded addon,
				// so a context-loss dispose here can race with that and double-dispose
				// the addon — xterm's WebglAddon throws on the second call because its
				// internal renderer reference is already torn down. Swallow it.
				webgl.onContextLoss(() => {
					try { webgl.dispose(); } catch { /* already disposed */ }
				});
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
		clearTimeout(reconnectTimer);
		resizeObserver?.disconnect();
		resizeObserver = undefined;
		document.removeEventListener('visibilitychange', onVisibility);
		window.removeEventListener('focus', maybeReconnect);
		window.removeEventListener('online', maybeReconnect);
		window.removeEventListener('beforeunload', persist);
		persist();
		for (const [, sock] of sockets) {
			sock.onclose = null;
			sock.onmessage = null;
			sock.onerror = null;
			sock.close();
		}
		sockets.clear();
		// Null addons and terminal reference before dispose so any inflight
		// callbacks (socket messages, resize observer races) are no-ops.
		const t = terminal;
		terminal = undefined;
		fit = undefined;
		search = undefined;
		serialize = undefined;
		// A prior context-loss event may have already disposed the WebGL addon;
		// xterm re-disposing it here would throw (see the webgl.onContextLoss
		// handler above), so don't let that take down the rest of cleanup.
		try { t?.dispose(); } catch { /* addon already disposed */ }
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
	{#if connState === 'reconnecting' || connState === 'failed'}
		<div class="connect-error" class:failed={connState === 'failed'}>
			<span>{connectError}</span>
			<button class="error-retry" onclick={() => { if (selectedPid) reconnectAttempts.delete(selectedPid); connState = 'reconnecting'; connectError = 'Reconnecting…'; if (selectedPid) openSocket(selectedPid); }}>Retry</button>
			{#if connState === 'failed'}
				<button class="error-dismiss" onclick={() => { connState = 'connected'; connectError = ''; }}>×</button>
			{/if}
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
		<label>
			<span>Working directory <span class="muted">(optional)</span></span>
			{#if workspacePaths}
				<div class="cwd-presets">
					<button type="button" class="preset-btn" class:active={newCwd === workspacePaths.home_dir} onclick={() => newCwd = newCwd === workspacePaths!.home_dir ? '' : workspacePaths!.home_dir}>
						~ Home
					</button>
					{#if workspacePaths.project_dir}
						<button type="button" class="preset-btn" class:active={newCwd === workspacePaths.project_dir} onclick={() => newCwd = newCwd === workspacePaths!.project_dir ? '' : workspacePaths!.project_dir!}>
							⎇ Project
						</button>
					{/if}
				</div>
			{/if}
			<input bind:value={newCwd} placeholder={workspacePaths?.home_dir ?? '~/project (default: home)'} />
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

	.connect-error { display: flex; align-items: center; gap: 0.5rem; padding: 0.35rem 0.75rem; background: #3d2000; color: #f5a623; font-size: 12px; border-bottom: 1px solid #7a4500; }
	.connect-error span { flex: 1; }
	.connect-error.failed { background: #3d0000; color: #f56262; border-color: #7a0000; }
	.error-retry { background: none; border: 1px solid currentColor; border-radius: 3px; color: inherit; cursor: pointer; font-size: 11px; padding: 0.1rem 0.5rem; flex-shrink: 0; opacity: 0.85; }
	.error-retry:hover { opacity: 1; }
	.error-dismiss { background: none; border: none; color: inherit; cursor: pointer; font-size: 16px; line-height: 1; padding: 0; flex-shrink: 0; opacity: 0.7; }
	.error-dismiss:hover { opacity: 1; }
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
	.cwd-presets { display: flex; gap: 0.4rem; margin-bottom: 0.35rem; }
	.preset-btn { font-size: 11px; padding: 0.2rem 0.6rem; border: 1px solid var(--color-border); border-radius: 4px; background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; font-family: var(--font-mono); }
	.preset-btn:hover { border-color: var(--color-accent); color: var(--color-accent); }
	.preset-btn.active { border-color: var(--color-accent); color: var(--color-accent); background: color-mix(in srgb, var(--color-accent) 12%, transparent); }
</style>

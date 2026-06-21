<script lang="ts">
	import { getAgentToken, getProject } from '$lib/remote/projects.remote';
	import { onMount, onDestroy } from 'svelte';
	import { Terminal } from '@xterm/xterm';
	import { FitAddon } from '@xterm/addon-fit';
	import '@xterm/xterm/css/xterm.css';
	import VirtualKeyboard from '$lib/terminal/VirtualKeyboard.svelte';
	import { layouts } from '$lib/terminal/keyboard';

	// Each project gets its own agent route (e.g. `/agent/<slug>`); the path is
	// published in the Project's status by the operator.
	let agentBase = $state('');

	let termEl: HTMLDivElement | undefined = $state();
	let newDialog: HTMLDialogElement | undefined = $state();
	let terminal: Terminal | undefined;
	let resizeObserver: ResizeObserver | undefined;
	let ws: WebSocket | null = null;
	let processes: any[] = $state([]);
	let selectedPid: string | null = $state(null);
	let newCmd = $state('');
	let newName = $state('');
	let newKind: 'one-shot' | 'persistent' = $state('one-shot');
	let agentToken: string | null = $state(null);

	// On touch devices we suppress the OS keyboard and drive input from our own
	// on-screen keyboard instead (Ctrl/Alt/Fn aren't reachable otherwise).
	let isTouch = $state(false);
	let fit: FitAddon | undefined;

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
		newCmd = ''; newName = ''; newKind = 'one-shot';
		newDialog?.showModal();
	}

	async function createProcess() {
		if (!agentToken || !agentBase || !newCmd.trim()) return;
		const res = await fetch(`${agentBase}/processes`, {
			method: 'POST',
			headers: { Authorization: `Bearer ${agentToken}`, 'Content-Type': 'application/json' },
			body: JSON.stringify({ name: newName || newCmd, command: newCmd, kind: newKind })
		});
		if (res.ok) {
			const p = await res.json();
			newCmd = ''; newName = '';
			newDialog?.close();
			await loadProcesses();
			attachToProcess(p.id);
		}
	}

	async function killProcess(id: string) {
		if (!agentToken) return;
		await fetch(`${agentBase}/processes/${id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${agentToken}` } });
		await loadProcesses();
		if (selectedPid === id) { ws?.close(); ws = null; selectedPid = null; }
	}

	function attachToProcess(id: string) {
		if (!agentToken || !agentBase) return;
		ws?.close();
		selectedPid = id;
		terminal?.clear();
		const wsUrl = `${agentBase.replace('https://', 'wss://').replace('http://', 'ws://')}/processes/${id}/output`;
		ws = new WebSocket(`${wsUrl}?token=${encodeURIComponent(agentToken)}`);
		// The agent streams output as binary frames; default binaryType is "blob",
		// which TextDecoder can't decode. Use arraybuffer so we can render it.
		ws.binaryType = 'arraybuffer';
		// On connect, refit and report the size so the PTY matches the viewport.
		ws.onopen = () => { fit?.fit(); sendResize(); };
		ws.onmessage = (e) => {
			const data = e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : e.data;
			terminal?.write(typeof data === 'string' ? data : new TextDecoder().decode(data));
		};
		ws.onclose = () => { selectedPid = null; };
	}

	onMount(async () => {
		try { agentToken = await getAgentToken(); } catch {}
		try {
			const project = await getProject();
			const path = project?.status?.agentPath;
			if (path) agentBase = `https://enzarb.dev${path}`;
		} catch {}
		isTouch = window.matchMedia('(pointer: coarse)').matches;
		terminal = new Terminal({ theme: { background: '#0f0f11', foreground: '#e8e8ed' }, fontFamily: 'JetBrains Mono, monospace' });
		fit = new FitAddon();
		terminal.loadAddon(fit);
		if (termEl) { terminal.open(termEl); fit.fit(); }
		terminal.onData((d: string) => send(d));
		// Keep xterm focusable (paste, cursor) but stop the OS keyboard from
		// covering the screen — our virtual keyboard takes over on touch.
		if (isTouch) {
			const ta = termEl?.querySelector('.xterm-helper-textarea');
			if (ta) ta.setAttribute('inputmode', 'none');
		}
		resizeObserver = new ResizeObserver(() => { fit?.fit(); sendResize(); });
		if (termEl) resizeObserver.observe(termEl);
		await loadProcesses();
	});

	onDestroy(() => { resizeObserver?.disconnect(); ws?.close(); terminal?.dispose(); });
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
		<button class="new-btn" title="New process" aria-label="New process" onclick={openNewDialog}>+</button>
	</div>
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
		<div class="dialog-actions">
			<button type="button" class="btn" onclick={() => newDialog?.close()}>Cancel</button>
			<button type="submit" class="btn btn-primary">Run</button>
		</div>
	</form>
</dialog>

<style>
	.terminal-page { display: grid; grid-template-rows: auto 1fr auto; gap: 0; height: calc(100vh - 200px); min-height: 400px; }
	@media (max-width: 640px) {
		.terminal-page { height: calc(100vh - 120px); }
	}

	/* On mobile the terminal goes full-bleed: break out of the page's
	   padding/header and let the on-screen keyboard own the bottom strip.
	   Pinned below the 52px mobile topbar so navigation stays reachable. */
	@media (max-width: 768px) {
		.terminal-page.touch {
			position: fixed;
			inset: 52px 0 0 0;
			height: auto;
			min-height: 0;
			z-index: 20;
			background: #0f0f11;
		}
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

	.term-container { background: #0f0f11; overflow: hidden; }
	.muted { color: var(--color-text-muted); font-size: 12px; padding: 0 0.75rem; align-self: center; }

	.new-dialog { border: 1px solid var(--color-border); border-radius: 8px; background: var(--color-surface); color: var(--color-text); padding: 0; min-width: 320px; }
	.new-dialog::backdrop { background: rgba(0, 0, 0, 0.5); }
	.new-form { display: flex; flex-direction: column; gap: 0.75rem; padding: 1.25rem; }
	.new-form h3 { margin: 0; font-size: 15px; }
	.new-form label { display: flex; flex-direction: column; gap: 0.25rem; font-size: 12px; font-weight: 500; }
	.new-form input, .new-form select { font-size: 13px; }
	.dialog-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.25rem; }
</style>

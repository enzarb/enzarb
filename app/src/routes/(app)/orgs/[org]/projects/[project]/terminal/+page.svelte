<script lang="ts">
	import type { PageData } from './$types';
	import { onMount, onDestroy } from 'svelte';
	let { data }: { data: PageData } = $props();

	let termEl: HTMLDivElement;
	let terminal: any;
	let ws: WebSocket | null = null;
	let processes: any[] = $state([]);
	let selectedPid: string | null = $state(null);
	let newCmd = $state('');
	let newName = $state('');
	let newKind: 'one-shot' | 'persistent' = $state('one-shot');

	async function loadProcesses() {
		if (!data.agentToken) return;
		const res = await fetch(`${data.agentBase}/processes`, {
			headers: { Authorization: `Bearer ${data.agentToken}` }
		});
		if (res.ok) processes = await res.json();
	}

	async function createProcess() {
		if (!data.agentToken || !newCmd.trim()) return;
		const res = await fetch(`${data.agentBase}/processes`, {
			method: 'POST',
			headers: { Authorization: `Bearer ${data.agentToken}`, 'Content-Type': 'application/json' },
			body: JSON.stringify({ name: newName || newCmd, command: newCmd, kind: newKind })
		});
		if (res.ok) {
			const p = await res.json();
			newCmd = ''; newName = '';
			await loadProcesses();
			attachToProcess(p.id);
		}
	}

	async function killProcess(id: string) {
		if (!data.agentToken) return;
		await fetch(`${data.agentBase}/processes/${id}`, {
			method: 'DELETE', headers: { Authorization: `Bearer ${data.agentToken}` }
		});
		await loadProcesses();
		if (selectedPid === id) { ws?.close(); ws = null; selectedPid = null; }
	}

	function attachToProcess(id: string) {
		if (!data.agentToken) return;
		ws?.close();
		selectedPid = id;
		const wsUrl = `${data.agentBase.replace('https://', 'wss://').replace('http://', 'ws://')}/processes/${id}/output`;
		ws = new WebSocket(`${wsUrl}?token=${encodeURIComponent(data.agentToken)}`);
		ws.onmessage = (e) => {
			const text = typeof e.data === 'string' ? e.data : new TextDecoder().decode(e.data);
			terminal?.write(text);
		};
		ws.onclose = () => { selectedPid = null; };
	}

	onMount(async () => {
		// Lazy-load xterm.js from CDN (no npm install needed in this phase)
		const { Terminal } = await import('https://cdn.jsdelivr.net/npm/@xterm/xterm@5/+esm' as any);
		const { FitAddon } = await import('https://cdn.jsdelivr.net/npm/@xterm/addon-fit@0/+esm' as any);
		terminal = new Terminal({ theme: { background: '#0f0f11', foreground: '#e8e8ed' }, fontFamily: 'JetBrains Mono, monospace' });
		const fit = new FitAddon();
		terminal.loadAddon(fit);
		terminal.open(termEl);
		fit.fit();
		terminal.onData((d: string) => ws?.send(d));
		await loadProcesses();
	});

	onDestroy(() => { ws?.close(); terminal?.dispose(); });
</script>

<div class="terminal-page">
	<div class="sidebar">
		<div class="new-process">
			<input bind:value={newName} placeholder="Name (optional)" />
			<input bind:value={newCmd} placeholder="Command" onkeydown={(e) => e.key === 'Enter' && createProcess()} />
			<select bind:value={newKind}>
				<option value="one-shot">One-shot</option>
				<option value="persistent">Persistent</option>
			</select>
			<button class="btn btn-primary" onclick={createProcess}>Run</button>
		</div>
		<div class="process-list">
			{#each processes as p}
				<div
					class="process-item {selectedPid === p.id ? 'active' : ''}"
					role="button" tabindex="0"
					onclick={() => attachToProcess(p.id)}
					onkeydown={(e) => e.key === 'Enter' && attachToProcess(p.id)}
				>
					<span class="proc-name">{p.name}</span>
					<span class="badge {p.status}">{p.status}</span>
					<button class="kill-btn" onclick={(e) => { e.stopPropagation(); killProcess(p.id); }}>×</button>
				</div>
			{:else}
				<p class="muted">No processes</p>
			{/each}
		</div>
	</div>
	<div class="term-container" bind:this={termEl}></div>
</div>

<style>
	.terminal-page { display: grid; grid-template-columns: 240px 1fr; gap: 0; height: calc(100vh - 200px); min-height: 400px; }
	.sidebar { border-right: 1px solid var(--color-border); display: flex; flex-direction: column; overflow: hidden; }
	.new-process { padding: 0.75rem; display: flex; flex-direction: column; gap: 0.5rem; border-bottom: 1px solid var(--color-border); }
	.process-list { flex: 1; overflow-y: auto; padding: 0.5rem; }
	.process-item { display: flex; align-items: center; gap: 0.5rem; width: 100%; padding: 0.375rem 0.5rem; border-radius: 4px; border: none; background: none; color: var(--color-text); font-size: 12px; cursor: pointer; text-align: left; }
	.process-item:hover { background: var(--color-surface-2); }
	.process-item.active { background: var(--color-accent-dim); }
	.proc-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
	.kill-btn { background: none; border: none; color: var(--color-text-muted); cursor: pointer; font-size: 16px; padding: 0 0.125rem; }
	.kill-btn:hover { color: var(--color-danger); }
	.term-container { background: #0f0f11; overflow: hidden; }
	.muted { color: var(--color-text-muted); font-size: 12px; padding: 0.5rem; }
</style>

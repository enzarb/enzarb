<script lang="ts">
	import { getAgentAuthToken } from '$lib/agentToken';

	interface Props {
		agentBase: string;
		namespace: string;
		project: string;
		regionKind: 'left' | 'right';
		onOpenFile?: (path: string, label: string) => void;
		onOpenTerminal?: (processId: string, label: string) => void;
		onOpenAgent?: (sessionId: string, label: string) => void;
		onCreateTerminal?: (processId: string, label: string) => void;
		onCreateAgent?: (sessionId: string, label: string) => void;
	}

	let {
		agentBase,
		namespace,
		project,
		regionKind,
		onOpenTerminal,
		onOpenAgent,
		onCreateTerminal,
		onCreateAgent
	}: Props = $props();

	type PaneType = 'terminal' | 'agent';

	let selectedType = $state<PaneType>('terminal');
	let processes: any[] = $state([]);
	let sessions: any[] = $state([]);
	let loading = $state(false);
	let err = $state('');

	// New terminal form
	let newCmd = $state('bash');
	let newName = $state('');
	let newKind: 'one-shot' | 'persistent' = $state('persistent');
	let creating = $state(false);
	let createErr = $state('');

	async function load() {
		loading = true;
		err = '';
		try {
			const token = await getAgentAuthToken(namespace, project);
			if (!token) { err = 'Not authenticated.'; return; }
			const auth = { Authorization: `Bearer ${token}` };
			if (selectedType === 'terminal') {
				const res = await fetch(`${agentBase}/processes`, { headers: auth });
				if (res.ok) processes = await res.json();
			} else {
				const res = await fetch(`${agentBase}/agent/sessions`, { headers: auth });
				if (res.ok) sessions = await res.json();
			}
		} catch {
			err = 'Failed to load.';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		selectedType;
		if (agentBase) load();
	});

	async function createTerminal() {
		createErr = '';
		creating = true;
		try {
			const token = await getAgentAuthToken(namespace, project);
			if (!token) { createErr = 'Not authenticated.'; return; }
			const parts = newCmd.trim().split(/\s+/);
			if (!parts[0]) { createErr = 'Enter a command.'; return; }
			const [command, ...args] = parts;
			const res = await fetch(`${agentBase}/processes`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ name: newName || newCmd.trim(), command, args, kind: newKind })
			});
			if (!res.ok) { createErr = `Failed (${res.status})`; return; }
			const p = await res.json();
			onCreateTerminal?.(p.id, p.name || newCmd.trim());
		} catch {
			createErr = 'Could not reach the workspace agent.';
		} finally {
			creating = false;
		}
	}

	async function createAgent() {
		createErr = '';
		creating = true;
		try {
			const token = await getAgentAuthToken(namespace, project);
			if (!token) { createErr = 'Not authenticated.'; return; }
			const res = await fetch(`${agentBase}/agent/sessions`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ label: 'New session' })
			});
			if (!res.ok) { createErr = `Failed (${res.status})`; return; }
			const s = await res.json();
			onCreateAgent?.(s.id, s.label || 'Agent session');
		} catch {
			createErr = 'Could not reach the workspace agent.';
		} finally {
			creating = false;
		}
	}
</script>

<div class="new-pane-dialog">
	{#if regionKind === 'right'}
		<div class="type-tabs">
			<button class="type-tab" class:active={selectedType === 'terminal'} onclick={() => selectedType = 'terminal'}>Terminal</button>
			<button class="type-tab" class:active={selectedType === 'agent'} onclick={() => selectedType = 'agent'}>Agent Session</button>
		</div>
	{/if}

	{#if err}
		<p class="err">{err}</p>
	{/if}

	{#if selectedType === 'terminal'}
		<div class="section">
			<div class="section-label">Open existing</div>
			{#if loading}
				<p class="muted">Loading…</p>
			{:else if processes.length === 0}
				<p class="muted">No running processes.</p>
			{:else}
				<div class="item-list">
					{#each processes as p}
						<button class="item" onclick={() => onOpenTerminal?.(p.id, p.name || p.id)}>
							<span class="status-dot {p.status}"></span>
							<span class="item-name">{p.name || p.id}</span>
							<span class="item-meta">{p.status}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
		<div class="section">
			<div class="section-label">New terminal</div>
			<label class="field">
				<span>Name <span class="muted-label">(optional)</span></span>
				<input bind:value={newName} placeholder="My process" />
			</label>
			<label class="field">
				<span>Command</span>
				<input bind:value={newCmd} placeholder="bash" />
			</label>
			<label class="field">
				<span>Kind</span>
				<select bind:value={newKind}>
					<option value="persistent">Persistent</option>
					<option value="one-shot">One-shot</option>
				</select>
			</label>
			{#if createErr}<p class="err">{createErr}</p>{/if}
			<button class="btn btn-primary" disabled={creating} onclick={createTerminal}>
				{creating ? 'Starting…' : 'Start terminal'}
			</button>
		</div>
	{:else}
		<div class="section">
			<div class="section-label">Open existing</div>
			{#if loading}
				<p class="muted">Loading…</p>
			{:else if sessions.length === 0}
				<p class="muted">No sessions yet.</p>
			{:else}
				<div class="item-list">
					{#each sessions as s}
						<button class="item" onclick={() => onOpenAgent?.(s.id, s.label || s.id)}>
							<span class="status-dot {s.status}"></span>
							<span class="item-name">{s.label || s.id}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
		<div class="section">
			<div class="section-label">New session</div>
			{#if createErr}<p class="err">{createErr}</p>{/if}
			<button class="btn btn-primary" disabled={creating} onclick={createAgent}>
				{creating ? 'Creating…' : 'New agent session'}
			</button>
		</div>
	{/if}
</div>

<style>
	.new-pane-dialog { display: flex; flex-direction: column; height: 100%; overflow-y: auto; padding: 0.75rem; gap: 0; }
	.type-tabs { display: flex; border-bottom: 1px solid var(--color-border); margin-bottom: 0.75rem; }
	.type-tab { flex: 1; padding: 0.4rem; font-size: 12px; border: none; background: none; color: var(--color-text-muted); cursor: pointer; border-bottom: 2px solid transparent; margin-bottom: -1px; }
	.type-tab:hover { color: var(--color-text); }
	.type-tab.active { color: var(--color-text); border-bottom-color: var(--color-accent); }
	.section { margin-bottom: 1rem; }
	.section-label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--color-text-muted); margin-bottom: 0.4rem; }
	.item-list { display: flex; flex-direction: column; gap: 1px; margin-bottom: 0.5rem; }
	.item { display: flex; align-items: center; gap: 0.4rem; padding: 0.35rem 0.5rem; border: 1px solid var(--color-border); border-radius: 4px; background: var(--color-surface); color: var(--color-text); font-size: 12px; cursor: pointer; text-align: left; }
	.item:hover { border-color: var(--color-accent); background: var(--color-surface-2); }
	.item-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
	.item-meta { font-size: 11px; color: var(--color-text-muted); flex-shrink: 0; }
	.status-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--color-text-muted); flex-shrink: 0; }
	.status-dot.running { background: #3fb950; }
	.status-dot.live { background: #3fb950; }
	.status-dot.exited, .status-dot.idle { background: var(--color-text-muted); }
	.status-dot.failed { background: var(--color-danger); }
	.field { display: flex; flex-direction: column; gap: 0.2rem; font-size: 12px; font-weight: 500; margin-bottom: 0.4rem; }
	.field input, .field select { font-size: 13px; padding: 0.3rem 0.5rem; background: var(--color-surface); color: var(--color-text); border: 1px solid var(--color-border); border-radius: 4px; }
	.muted { color: var(--color-text-muted); font-size: 12px; }
	.muted-label { font-weight: 400; color: var(--color-text-muted); }
	.err { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0; }
	.btn { padding: 0.4rem 0.75rem; font-size: 12px; border: 1px solid var(--color-border); border-radius: 4px; cursor: pointer; background: var(--color-surface); color: var(--color-text); }
	.btn-primary { background: var(--color-accent); color: #fff; border-color: var(--color-accent); }
	.btn-primary:hover { opacity: 0.9; }
	.btn-primary:disabled { opacity: 0.5; cursor: default; }
</style>

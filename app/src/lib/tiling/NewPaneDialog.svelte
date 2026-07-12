<script lang="ts">
	import { untrack } from 'svelte';
	import { getAgentAuthToken } from '$lib/agentToken';
	import { loadLastCwd, saveLastCwd, getProjectColor } from '$lib/tiling/layout';

	type ProjectRef = { namespace: string; project: string };

	interface Props {
		regionKind: 'left' | 'right';
		getAgentBase: (ns: string, proj: string) => string;
		ensureAgentBase: (ns: string, proj: string) => Promise<string>;
		global: boolean;
		orgProjects?: Record<string, { slug: string; displayName: string }[]>;
		defaultRef: ProjectRef | null;
		onOpenFile?: (path: string, label: string, namespace: string, project: string) => void;
		onOpenTerminal?: (processId: string, label: string, namespace: string, project: string) => void;
		onOpenAgent?: (sessionId: string, label: string, namespace: string, project: string) => void;
		onCreateTerminal?: (processId: string, label: string, namespace: string, project: string) => void;
		onCreateAgent?: (sessionId: string, label: string, namespace: string, project: string) => void;
	}

	let {
		regionKind,
		getAgentBase,
		ensureAgentBase,
		global,
		orgProjects,
		defaultRef,
		onOpenTerminal,
		onOpenAgent,
		onCreateTerminal,
		onCreateAgent
	}: Props = $props();

	type PaneType = 'terminal' | 'agent';

	// The project this new pane targets. Global mode lets the user pick any
	// project; single-project mode pins it to the seed project (defaultRef).
	let selectedRef = $state<ProjectRef | null>(untrack(() => defaultRef));
	// A flat list of every org/project for the picker.
	const projectOptions = $derived(
		Object.entries(orgProjects ?? {}).flatMap(([ns, projects]) =>
			projects.map((p) => ({ namespace: ns, project: p.slug, displayName: p.displayName }))
		)
	);
	// Show the picker whenever we have a list to choose from (global mode).
	const showPicker = $derived(projectOptions.length > 0);

	const agentBase = $derived(selectedRef ? getAgentBase(selectedRef.namespace, selectedRef.project) : '');

	let selectedType = $state<PaneType>('terminal');
	let processes: any[] = $state([]);
	let sessions: any[] = $state([]);
	let loading = $state(false);
	let err = $state('');

	// New terminal form
	let newCmd = $state('bash');
	let newName = $state('');
	let newKind: 'one-shot' | 'persistent' = $state('persistent');
	let newCwd = $state(untrack(() => (defaultRef ? loadLastCwd(defaultRef.namespace, defaultRef.project) : '')));
	let creating = $state(false);
	let createErr = $state('');
	let workspacePaths: { home_dir: string; project_dir: string | null } | null = $state(null);

	// Resolve the selected project's agentBase (may not be cached yet in global mode).
	$effect(() => {
		if (selectedRef) ensureAgentBase(selectedRef.namespace, selectedRef.project);
	});

	function pickProject(value: string) {
		const [ns, ...rest] = value.split('/');
		const proj = rest.join('/');
		selectedRef = { namespace: ns, project: proj };
		// Reset per-project state so nothing leaks across projects.
		workspacePaths = null;
		processes = [];
		sessions = [];
		newCwd = loadLastCwd(ns, proj);
	}

	async function load() {
		if (!selectedRef || !agentBase) return;
		const ref = selectedRef;
		loading = true;
		err = '';
		try {
			const token = await getAgentAuthToken(ref.namespace, ref.project);
			if (!token) { err = 'Not authenticated.'; return; }
			const auth = { Authorization: `Bearer ${token}` };
			if (!workspacePaths) {
				const statusRes = await fetch(`${agentBase}/status`, { headers: auth });
				if (statusRes.ok) workspacePaths = await statusRes.json();
			}
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
		if (!selectedRef) return;
		const ref = selectedRef;
		createErr = '';
		creating = true;
		try {
			const token = await getAgentAuthToken(ref.namespace, ref.project);
			if (!token) { createErr = 'Not authenticated.'; return; }
			const parts = newCmd.trim().split(/\s+/);
			if (!parts[0]) { createErr = 'Enter a command.'; return; }
			const [command, ...args] = parts;
			const res = await fetch(`${agentBase}/processes`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ name: newName || newCmd.trim(), command, args, kind: newKind, ...(newCwd ? { cwd: newCwd } : {}) })
			});
			if (!res.ok) { createErr = `Failed (${res.status})`; return; }
			const p = await res.json();
			saveLastCwd(ref.namespace, ref.project, newCwd);
			onCreateTerminal?.(p.id, p.name || newCmd.trim(), ref.namespace, ref.project);
		} catch {
			createErr = 'Could not reach the workspace agent.';
		} finally {
			creating = false;
		}
	}

	async function createAgent() {
		if (!selectedRef) return;
		const ref = selectedRef;
		createErr = '';
		creating = true;
		try {
			const token = await getAgentAuthToken(ref.namespace, ref.project);
			if (!token) { createErr = 'Not authenticated.'; return; }
			const res = await fetch(`${agentBase}/agent/sessions`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ label: 'New session', ...(newCwd ? { cwd: newCwd } : {}) })
			});
			if (!res.ok) { createErr = `Failed (${res.status})`; return; }
			const s = await res.json();
			saveLastCwd(ref.namespace, ref.project, newCwd);
			onCreateAgent?.(s.id, s.label || 'Agent session', ref.namespace, ref.project);
		} catch {
			createErr = 'Could not reach the workspace agent.';
		} finally {
			creating = false;
		}
	}
</script>

{#snippet cwdField()}
	<label class="field">
		<span>Working directory <span class="muted-label">(optional)</span></span>
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
{/snippet}

<div class="new-pane-dialog">
	{#if showPicker}
		<label class="field project-picker">
			<span>Project</span>
			<div class="picker-row">
				{#if selectedRef}
					<span class="project-swatch" style="background: {getProjectColor(selectedRef.namespace, selectedRef.project)}"></span>
				{/if}
				<select
					value={selectedRef ? `${selectedRef.namespace}/${selectedRef.project}` : ''}
					onchange={(e) => pickProject(e.currentTarget.value)}
				>
					{#if !selectedRef}<option value="" disabled>Choose a project…</option>{/if}
					{#each projectOptions as opt}
						<option value="{opt.namespace}/{opt.project}">{opt.namespace} / {opt.displayName}</option>
					{/each}
				</select>
			</div>
		</label>
	{/if}

	{#if !selectedRef}
		<p class="muted">Select a project to add a pane.</p>
	{:else}
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
						<button class="item" onclick={() => onOpenTerminal?.(p.id, p.name || p.id, selectedRef!.namespace, selectedRef!.project)}>
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
			{@render cwdField()}
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
						<button class="item" onclick={() => onOpenAgent?.(s.id, s.label || s.id, selectedRef!.namespace, selectedRef!.project)}>
							<span class="status-dot {s.status}"></span>
							<span class="item-name">{s.label || s.id}</span>
						</button>
					{/each}
				</div>
			{/if}
		</div>
		<div class="section">
			<div class="section-label">New session</div>
			{@render cwdField()}
			{#if createErr}<p class="err">{createErr}</p>{/if}
			<button class="btn btn-primary" disabled={creating} onclick={createAgent}>
				{creating ? 'Creating…' : 'New agent session'}
			</button>
		</div>
	{/if}
	{/if}
</div>

<style>
	.new-pane-dialog { display: flex; flex-direction: column; height: 100%; overflow-y: auto; padding: 0.75rem; gap: 0; }
	.project-picker { margin-bottom: 0.75rem; }
	.picker-row { display: flex; align-items: center; gap: 0.4rem; }
	.picker-row select { flex: 1; }
	.project-swatch { width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0; }
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
	.cwd-presets { display: flex; gap: 0.3rem; margin-bottom: 0.2rem; }
	.preset-btn { font-size: 11px; padding: 0.2rem 0.5rem; border: 1px solid var(--color-border); border-radius: 4px; background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.preset-btn:hover { color: var(--color-text); }
	.preset-btn.active { color: #fff; background: var(--color-accent); border-color: var(--color-accent); }
	.muted { color: var(--color-text-muted); font-size: 12px; }
	.muted-label { font-weight: 400; color: var(--color-text-muted); }
	.err { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0; }
	.btn { padding: 0.4rem 0.75rem; font-size: 12px; border: 1px solid var(--color-border); border-radius: 4px; cursor: pointer; background: var(--color-surface); color: var(--color-text); }
	.btn-primary { background: var(--color-accent); color: #fff; border-color: var(--color-accent); }
	.btn-primary:hover { opacity: 0.9; }
	.btn-primary:disabled { opacity: 0.5; cursor: default; }
</style>

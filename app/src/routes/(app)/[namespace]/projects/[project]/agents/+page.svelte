<script lang="ts">
	import { getProject } from '$lib/remote/projects.remote';
	import { getAgentAuthToken } from '$lib/agentToken';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { SessionMeta } from '$lib/agent/types';
	import Tooltip from '$lib/components/Tooltip.svelte';

	let agentBase = $state('');
	let sessions: SessionMeta[] = $state([]);
	let loading = $state(true);
	let loadError = $state('');
	let creating = $state(false);
	let showNewForm = $state(false);
	let newCwd = $state('~');
	let confirmDelete = $state<string | null>(null);

	const base = `/${page.params.namespace}/projects/${page.params.project}/agents`;

	async function loadSessions() {
		loadError = '';
		if (!agentBase) return;
		const token = await getAgentAuthToken(page.params.namespace!, page.params.project!);
		if (!token) {
			loadError = 'Session expired — please reload the page to sign in again.';
			return;
		}
		try {
			const res = await fetch(`${agentBase}/agent/sessions`, {
				headers: { Authorization: `Bearer ${token}` }
			});
			if (res.ok) sessions = await res.json();
			else loadError = `Failed to load sessions (${res.status}).`;
		} catch {
			loadError = 'Could not reach the workspace agent.';
		}
		loading = false;
	}

	async function createSession() {
		if (!agentBase || creating) return;
		creating = true;
		loadError = '';
		try {
			const token = await getAgentAuthToken(page.params.namespace!, page.params.project!);
			if (!token) {
				loadError = 'Session expired — please reload the page to sign in again.';
				return;
			}
			const cwd = newCwd.trim() || '~';
			const res = await fetch(`${agentBase}/agent/sessions`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({ cwd })
			});
			if (res.ok) {
				const session = await res.json();
				goto(`${base}/${session.id}`);
			} else {
				loadError = `Failed to start session (${res.status}).`;
			}
		} catch {
			loadError = 'Could not reach the workspace agent.';
		} finally {
			creating = false;
			showNewForm = false;
		}
	}

	async function archiveSession(id: string) {
		const token = await getAgentAuthToken(page.params.namespace!, page.params.project!);
		if (!token) return;
		await fetch(`${agentBase}/agent/sessions/${id}`, {
			method: 'DELETE',
			headers: { Authorization: `Bearer ${token}` }
		});
		sessions = sessions.filter((s) => s.id !== id);
		confirmDelete = null;
	}

	onMount(async () => {
		try {
			const project = await getProject();
			const path = project?.status?.agentPath;
			if (path) {
				agentBase = `https://enzarb.dev${path}`;
			} else {
				loadError = 'Workspace agent path is not set — the workspace may still be provisioning.';
			}
		} catch {
			loadError = 'Could not load project — please reload the page.';
		}
		await loadSessions();
	});
</script>

<div class="agents-page">
	<div class="agents-header">
		<h3>Agent sessions</h3>
		{#if !showNewForm}
			<button class="btn btn-primary" onclick={() => showNewForm = true} disabled={!agentBase} title={!agentBase ? 'Workspace is not ready yet' : undefined}>
				+ New session
			</button>
		{/if}
	</div>

	{#if showNewForm}
		<form class="new-session-form" onsubmit={(e) => { e.preventDefault(); createSession(); }}>
			<label class="cwd-label" for="new-cwd">Working directory</label>
			<div class="new-session-row">
				<input
					id="new-cwd"
					class="cwd-input"
					type="text"
					bind:value={newCwd}
					placeholder="~"
					spellcheck={false}
					autocomplete="off"
				/>
				<button type="submit" class="btn btn-primary" disabled={creating || !agentBase} title={!agentBase ? 'Workspace is not ready yet' : undefined}>
					{creating ? 'Starting…' : 'Start'}
				</button>
				<button type="button" class="btn" onclick={() => { showNewForm = false; newCwd = '~'; }}>
					Cancel
				</button>
			</div>
		</form>
	{/if}

	{#if loading}
		<p class="muted">Loading sessions…</p>
	{:else if loadError}
		<p class="error">{loadError}</p>
	{:else if !sessions.length}
		<p class="muted">No sessions yet — start one to chat with Claude Code about this project.</p>
	{:else}
		<div class="session-list">
			{#each sessions as s (s.id)}
				{@const hasMeta = s._meta && Object.keys(s._meta).length > 0}
				<div class="session-row">
					<a class="session-link" href="{base}/{s.id}">
						<span class="status-dot {s.status}"></span>
						<Tooltip placement="bottom">
							{#snippet children()}<span class="session-label">{s.label}</span>{/snippet}
							{#snippet content()}<span class="label-tooltip">{s.label}</span>{/snippet}
						</Tooltip>
						<span class="session-time">{s.updated_at ? new Date(s.updated_at).toLocaleString() : ''}</span>
						{#if hasMeta}
							<Tooltip placement="bottom">
								{#snippet children()}<span class="meta-badge">meta</span>{/snippet}
								{#snippet content()}
									<pre class="meta-pre">{JSON.stringify(s._meta, null, 2)}</pre>
								{/snippet}
							</Tooltip>
						{/if}
					</a>
					{#if confirmDelete === s.id}
						<span class="confirm-row">
							<span class="confirm-text">Delete?</span>
							<button class="btn-danger-sm" onclick={() => archiveSession(s.id)}>Yes</button>
							<button class="btn-ghost-sm" onclick={() => (confirmDelete = null)}>No</button>
						</span>
					{:else}
						<button
							class="delete-btn"
							title="Delete session"
							onclick={(e) => { e.preventDefault(); confirmDelete = s.id; }}
						>✕</button>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>

<style>
	.agents-page { padding: 0.5rem 0; }
	.agents-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 1rem; }
	.new-session-form { margin-bottom: 1rem; }
	.cwd-label { display: block; font-size: 12px; color: var(--color-text-muted); margin-bottom: 0.35rem; }
	.new-session-row { display: flex; gap: 0.5rem; align-items: center; }
	.cwd-input { flex: 1; font-family: var(--font-mono); font-size: 13px; padding: 0.4rem 0.6rem; border: 1px solid var(--color-border); border-radius: 6px; background: var(--color-surface); color: var(--color-text); min-width: 0; }
	.agents-header h3 { margin: 0; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.error { color: var(--color-danger); font-size: 13px; }
	.session-list { display: flex; flex-direction: column; border: 1px solid var(--color-border); border-radius: 6px; overflow: hidden; }
	.session-row { display: flex; align-items: center; border-bottom: 1px solid var(--color-border); }
	.session-row:last-child { border-bottom: none; }
	.session-row:hover { background: var(--color-surface-2); }
	.session-link { display: flex; align-items: center; gap: 0.6rem; padding: 0.6rem 0.9rem; flex: 1; text-decoration: none; color: var(--color-text); min-width: 0; }
	.status-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--color-text-muted); flex-shrink: 0; }
	.status-dot.live { background: #3fb950; }
	.session-label { flex: 1; font-family: var(--font-mono); font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
	.meta-badge { font-size: 10px; font-family: var(--font-mono); padding: 1px 5px; border-radius: 3px; background: var(--color-surface-2); color: var(--color-text-muted); border: 1px solid var(--color-border); flex-shrink: 0; cursor: help; }
	.meta-pre { margin: 0; font-family: var(--font-mono); font-size: 12px; white-space: pre; }
	.label-tooltip { font-family: var(--font-mono); font-size: 12px; word-break: break-all; }
	.session-time { font-size: 11px; color: var(--color-text-muted); flex-shrink: 0; }
	.delete-btn { background: none; border: none; cursor: pointer; color: var(--color-text-muted); font-size: 13px; padding: 0.6rem 0.75rem; line-height: 1; opacity: 0; transition: opacity 0.1s; }
	.session-row:hover .delete-btn { opacity: 1; }
	.delete-btn:hover { color: var(--color-danger); }
	.confirm-row { display: flex; align-items: center; gap: 0.4rem; padding: 0 0.6rem; flex-shrink: 0; }
	.confirm-text { font-size: 12px; color: var(--color-text-muted); }
	.btn-danger-sm { font-size: 12px; padding: 0.2rem 0.5rem; border-radius: 4px; border: 1px solid var(--color-danger); background: none; color: var(--color-danger); cursor: pointer; }
	.btn-danger-sm:hover { background: var(--color-danger); color: #fff; }
	.btn-ghost-sm { font-size: 12px; padding: 0.2rem 0.5rem; border-radius: 4px; border: 1px solid var(--color-border); background: none; color: var(--color-text-muted); cursor: pointer; }
</style>

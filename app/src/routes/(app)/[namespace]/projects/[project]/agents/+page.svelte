<script lang="ts">
	import { getProject } from '$lib/remote/projects.remote';
	import { getAgentAuthToken } from '$lib/agentToken';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import type { SessionMeta } from '$lib/agent/types';

	let agentBase = $state('');
	let sessions: SessionMeta[] = $state([]);
	let loading = $state(true);
	let loadError = $state('');
	let creating = $state(false);

	const base = `/${page.params.namespace}/projects/${page.params.project}/agents`;

	async function loadSessions() {
		loadError = '';
		if (!agentBase) return;
		const token = await getAgentAuthToken();
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
		try {
			const token = await getAgentAuthToken();
			if (!token) return;
			const res = await fetch(`${agentBase}/agent/sessions`, {
				method: 'POST',
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
				body: JSON.stringify({})
			});
			if (res.ok) {
				const session = await res.json();
				goto(`${base}/${session.id}`);
			}
		} finally {
			creating = false;
		}
	}

	onMount(async () => {
		try {
			const project = await getProject();
			const path = project?.status?.agentPath;
			if (path) agentBase = `https://enzarb.dev${path}`;
		} catch {}
		await loadSessions();
	});
</script>

<div class="agents-page">
	<div class="agents-header">
		<h3>Agent sessions</h3>
		<button class="btn btn-primary" onclick={createSession} disabled={creating || !agentBase}>
			{creating ? 'Starting…' : '+ New session'}
		</button>
	</div>

	{#if loading}
		<p class="muted">Loading sessions…</p>
	{:else if loadError}
		<p class="error">{loadError}</p>
	{:else if !sessions.length}
		<p class="muted">No sessions yet — start one to chat with Claude Code about this project.</p>
	{:else}
		<div class="session-list">
			{#each sessions as s (s.id)}
				<a class="session-row" href="{base}/{s.id}">
					<span class="status-dot {s.status}"></span>
					<span class="session-label">{s.label}</span>
					<span class="session-time">{new Date(s.updated_at).toLocaleString()}</span>
				</a>
			{/each}
		</div>
	{/if}
</div>

<style>
	.agents-page { padding: 0.5rem 0; }
	.agents-header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 1rem; }
	.agents-header h3 { margin: 0; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.error { color: var(--color-danger); font-size: 13px; }
	.session-list { display: flex; flex-direction: column; border: 1px solid var(--color-border); border-radius: 6px; overflow: hidden; }
	.session-row { display: flex; align-items: center; gap: 0.6rem; padding: 0.6rem 0.9rem; border-bottom: 1px solid var(--color-border); text-decoration: none; color: var(--color-text); }
	.session-row:last-child { border-bottom: none; }
	.session-row:hover { background: var(--color-surface-2); }
	.status-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--color-text-muted); flex-shrink: 0; }
	.status-dot.live { background: #3fb950; }
	.session-label { flex: 1; font-family: var(--font-mono); font-size: 13px; }
	.session-time { font-size: 11px; color: var(--color-text-muted); }
</style>

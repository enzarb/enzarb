<script lang="ts">
	import { getAgentToken, getProject } from '$lib/remote/projects.remote';

	// Per-project agent route (`/agent/<slug>`), published in the Project status.
	let agentBase = $state('');

	// Curated quick-add catalog (mise short names).
	const curated = [
		{ name: 'claude', label: 'Claude Code' },
		{ name: 'node', label: 'Node.js' },
		{ name: 'python', label: 'Python' },
		{ name: 'go', label: 'Go' },
		{ name: 'rust', label: 'Rust' },
		{ name: 'helm', label: 'Helm' },
		{ name: 'kubectl', label: 'kubectl' },
		{ name: 'terraform', label: 'Terraform' }
	];

	type InstalledTool = {
		name: string;
		version: string | null;
		requested: string | null;
		installed: boolean;
		active: boolean;
	};
	type RegistryTool = { short: string; full: string };

	let token: string | null = $state(null);
	let installed: InstalledTool[] = $state([]);
	let registry: RegistryTool[] = $state([]);
	let loading = $state(true);
	let busy: string | null = $state(null);
	let errorMsg: string | null = $state(null);

	let search = $state('');
	let versionInput = $state('latest');
	let versionOptions: string[] = $state([]);

	function authHeaders(): Record<string, string> {
		return { Authorization: `Bearer ${token}` };
	}

	async function loadInstalled() {
		const res = await fetch(`${agentBase}/tools`, { headers: authHeaders() });
		if (res.ok) installed = await res.json();
	}

	async function loadRegistry() {
		const res = await fetch(`${agentBase}/tools/registry`, { headers: authHeaders() });
		if (res.ok) registry = await res.json();
	}

	async function init() {
		loading = true;
		errorMsg = null;
		try {
			const [agentToken, project] = await Promise.all([getAgentToken(), getProject()]);
			token = agentToken;
			const path = project?.status?.agentPath;
			if (!path) throw new Error('agent not ready');
			agentBase = `https://enzarb.dev${path}`;
			await Promise.all([loadInstalled(), loadRegistry()]);
		} catch {
			errorMsg = 'Agent not available — the project may still be provisioning.';
		} finally {
			loading = false;
		}
	}

	const installedNames = $derived(new Set(installed.map((t) => t.name)));

	// Registry matches for the search box: only when searching, capped for speed.
	const matches = $derived(
		search.trim().length < 2
			? []
			: registry
					.filter(
						(t) =>
							t.short.toLowerCase().includes(search.toLowerCase()) ||
							t.full.toLowerCase().includes(search.toLowerCase())
					)
					.slice(0, 25)
	);

	async function addTool(name: string, version = 'latest') {
		if (!token) return;
		busy = name;
		errorMsg = null;
		try {
			const res = await fetch(`${agentBase}/tools`, {
				method: 'POST',
				headers: { ...authHeaders(), 'Content-Type': 'application/json' },
				body: JSON.stringify({ name, version })
			});
			if (!res.ok) {
				errorMsg = `Failed to install ${name}: ${await res.text()}`;
			} else {
				search = '';
				versionInput = 'latest';
				versionOptions = [];
				await loadInstalled();
			}
		} finally {
			busy = null;
		}
	}

	async function removeTool(name: string) {
		if (!token) return;
		busy = name;
		errorMsg = null;
		try {
			const res = await fetch(`${agentBase}/tools/${encodeURIComponent(name)}`, {
				method: 'DELETE',
				headers: authHeaders()
			});
			if (!res.ok) errorMsg = `Failed to remove ${name}: ${await res.text()}`;
			else await loadInstalled();
		} finally {
			busy = null;
		}
	}

	async function loadVersions(name: string) {
		if (!token) return;
		versionOptions = [];
		const res = await fetch(`${agentBase}/tools/${encodeURIComponent(name)}/versions`, {
			headers: authHeaders()
		});
		if (res.ok) {
			const all: string[] = await res.json();
			// Newest last from mise; show newest first, capped.
			versionOptions = all.slice(-50).reverse();
		}
	}
</script>

<svelte:head><title>Tools</title></svelte:head>

{#await init()}
	<p class="muted">Loading…</p>
{:then}
	{#if errorMsg && !token}
		<p class="muted">{errorMsg}</p>
	{:else}
		<div class="tools-page">
			{#if errorMsg}
				<div class="error">{errorMsg}</div>
			{/if}

			<section>
				<h3>Installed</h3>
				{#if loading}
					<p class="muted">Loading…</p>
				{:else if installed.length === 0}
					<p class="muted">No tools configured yet. Add one below.</p>
				{:else}
					<table class="tool-table">
						<thead><tr><th>Tool</th><th>Requested</th><th>Resolved</th><th>Status</th><th></th></tr></thead>
						<tbody>
							{#each installed as t}
								<tr>
									<td class="mono">{t.name}</td>
									<td class="muted mono">{t.requested ?? '—'}</td>
									<td class="muted mono">{t.version ?? '—'}</td>
									<td>
										{#if t.installed}
											<span class="pill ok">installed</span>
										{:else}
											<span class="pill pending">pending</span>
										{/if}
									</td>
									<td>
										<button class="btn danger" disabled={busy === t.name} onclick={() => removeTool(t.name)}>
											{busy === t.name ? '…' : 'Remove'}
										</button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			</section>

			<section>
				<h3>Quick add</h3>
				<div class="chip-row">
					{#each curated as c}
						<button
							class="chip"
							disabled={installedNames.has(c.name) || busy === c.name}
							onclick={() => addTool(c.name)}
							title={installedNames.has(c.name) ? 'Already installed' : `Install ${c.label}`}
						>
							{busy === c.name ? '…' : c.label}
						</button>
					{/each}
				</div>
			</section>

			<section>
				<h3>Search the registry</h3>
				<input
					class="search"
					type="text"
					placeholder="Search {registry.length} tools (e.g. ripgrep, deno)…"
					bind:value={search}
				/>
				{#if search.trim().length >= 2}
					<table class="tool-table">
						<tbody>
							{#each matches as m}
								<tr>
									<td class="mono">{m.short}</td>
									<td class="muted mono">{m.full}</td>
									<td>
										<button class="btn" disabled={installedNames.has(m.short) || busy === m.short} onclick={() => loadVersions(m.short)}>Versions</button>
									</td>
									<td>
										<button class="btn primary" disabled={installedNames.has(m.short) || busy === m.short} onclick={() => addTool(m.short)}>
											{busy === m.short ? '…' : installedNames.has(m.short) ? 'Installed' : 'Add latest'}
										</button>
									</td>
								</tr>
							{:else}
								<tr><td class="muted">No matches.</td></tr>
							{/each}
						</tbody>
					</table>

					{#if versionOptions.length > 0}
						<div class="version-add">
							<input class="search" list="version-list" bind:value={versionInput} placeholder="version" />
							<datalist id="version-list">
								{#each versionOptions as v}<option value={v}></option>{/each}
							</datalist>
							<button class="btn primary" disabled={!search.trim() || busy !== null} onclick={() => addTool(search.trim().split(/\s+/)[0], versionInput)}>
								Add pinned version
							</button>
						</div>
					{/if}
				{/if}
			</section>

			<p class="hint">Changes apply live in the workspace via <span class="mono">mise</span>. The workspace's <span class="mono">mise.toml</span> is the source of truth — no restart needed.</p>
		</div>
	{/if}
{/await}

<style>
	.tools-page { display: flex; flex-direction: column; gap: 1.75rem; max-width: 760px; }
	section { display: flex; flex-direction: column; gap: 0.5rem; }
	h3 { font-size: 13px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--color-text-muted); margin: 0; }
	.tool-table { width: 100%; }
	.tool-table th { text-align: left; font-size: 11px; text-transform: uppercase; color: var(--color-text-muted); font-weight: 500; }
	.mono { font-family: var(--font-mono); font-size: 13px; }
	.muted { color: var(--color-text-muted); }
	.chip-row { display: flex; flex-wrap: wrap; gap: 0.5rem; }
	.chip { padding: 0.35rem 0.75rem; border: 1px solid var(--color-border); border-radius: 999px; background: none; color: var(--color-text); font-size: 13px; cursor: pointer; }
	.chip:hover:not(:disabled) { border-color: var(--color-accent); color: var(--color-accent); }
	.chip:disabled { opacity: 0.4; cursor: default; }
	.search { width: 100%; padding: 0.5rem 0.75rem; border: 1px solid var(--color-border); border-radius: 6px; background: var(--color-bg); color: var(--color-text); font-size: 13px; }
	.btn { padding: 0.25rem 0.6rem; border: 1px solid var(--color-border); border-radius: 4px; background: none; color: var(--color-text); font-size: 12px; cursor: pointer; }
	.btn:disabled { opacity: 0.4; cursor: default; }
	.btn.primary { border-color: var(--color-accent); color: var(--color-accent); }
	.btn.danger:hover:not(:disabled) { border-color: #c0392b; color: #c0392b; }
	.pill { font-size: 11px; padding: 0.1rem 0.4rem; border-radius: 4px; }
	.pill.ok { background: rgba(46, 160, 67, 0.15); color: #2ea043; }
	.pill.pending { background: rgba(210, 153, 34, 0.15); color: #d29922; }
	.version-add { display: flex; gap: 0.5rem; align-items: center; margin-top: 0.5rem; }
	.error { padding: 0.5rem 0.75rem; border: 1px solid #c0392b; border-radius: 6px; color: #e06c5d; font-size: 13px; }
	.hint { font-size: 12px; color: var(--color-text-muted); }
</style>

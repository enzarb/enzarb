<script lang="ts">
	import { getProject, getAgentToken } from '$lib/remote/projects.remote';
	import { getEnvironments, createEnv, addDomain, setDefaultEnv } from '$lib/remote/environments.remote';
	import { getProjectRepoDetails } from '$lib/remote/registry.remote';
	import { page } from '$app/state';

	function formatBytes(bytes: number): string {
		if (bytes >= 1073741824) return (bytes / 1073741824).toFixed(1) + ' GiB';
		if (bytes >= 1048576) return (bytes / 1048576).toFixed(0) + ' MiB';
		return (bytes / 1024).toFixed(0) + ' KiB';
	}

	async function fetchDiskUsage(agentPath: string, token: string) {
		const res = await fetch(`https://enzarb.dev${agentPath}/status`, {
			headers: { Authorization: `Bearer ${token}` }
		});
		if (!res.ok) return null;
		const data = await res.json();
		return data.disk as { used_bytes: number; total_bytes: number };
	}

	let showNewEnv = $state(false);
	let domainEnv: string | null = $state(null);
	let envRefresh = $state(0);
	let openDropdown: string | null = $state(null);
	const domainForm = $derived(domainEnv ? addDomain.for(domainEnv) : null);
	let copiedNs: string | null = $state(null);
	const registryPrefix = $derived(`registry.enzarb.dev/${page.params.namespace}/${page.params.project}`);

	async function handleSetDefault(slug: string | null) {
		await setDefaultEnv({ envSlug: slug });
		envRefresh++;
		openDropdown = null;
	}

	async function copyNs(ns: string) {
		await navigator.clipboard.writeText(ns);
		copiedNs = ns;
		setTimeout(() => { copiedNs = null; }, 1500);
	}

	function toggleDropdown(name: string) {
		openDropdown = openDropdown === name ? null : name;
	}
</script>

{#await Promise.all([getProject(), getAgentToken()]) then [project, token]}
	<div class="overview">
		<div class="card storage-card">
			<div class="card-label">Storage</div>
			<code class="mono">{project.spec.storage?.size ?? '–'}</code>
			{#if project.status?.agentPath}
				{#await fetchDiskUsage(project.status.agentPath, token) then disk}
					{#if disk && disk.total_bytes > 0}
						{@const pct = Math.round((disk.used_bytes / disk.total_bytes) * 100)}
						<div class="disk-bar-wrap">
							<div class="disk-bar" style="width:{pct}%" class:disk-warn={pct > 80}></div>
						</div>
						<div class="disk-label">{formatBytes(disk.used_bytes)} used of {formatBytes(disk.total_bytes)}</div>
					{/if}
				{/await}
			{/if}
		</div>

		{#if project.status?.conditions?.length}
			<div class="conditions card">
				<h3>Conditions</h3>
				<table>
					<thead><tr><th>Type</th><th>Status</th><th>Message</th></tr></thead>
					<tbody>
						{#each project.status.conditions as cond}
							<tr>
								<td>{cond.type}</td>
								<td><span class="badge {cond.status === 'True' ? 'running' : 'error'}">{cond.status}</span></td>
								<td class="muted">{cond.message ?? ''}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<div class="two-col">
			<div class="card images-section">
				<div class="card-label">Images</div>
				{#await getProjectRepoDetails() then repos}
					{#if repos.length === 0}
						<p class="muted empty-msg">No images yet.<br/><code class="mono small">{registryPrefix}</code></p>
					{:else}
						<div class="image-list">
							{#each repos as repo}
								{@const shortName = repo.name.replace(`${page.params.namespace}/${page.params.project}`, '')}
								<div class="image-row">
									<span class="image-name mono">{shortName || '(root)'}</span>
									<span class="image-meta muted">{repo.tagCount} {repo.tagCount === 1 ? 'tag' : 'tags'}</span>
								</div>
							{/each}
						</div>
					{/if}
				{:catch}
					<p class="muted">Could not load images.</p>
				{/await}
			</div>

			<div class="card env-section">
				<div class="env-section-header">
					<span class="card-label">Environments</span>
					<button class="btn btn-sm btn-subtle" onclick={() => (showNewEnv = !showNewEnv)} title="New environment">+</button>
				</div>

				{#if showNewEnv}
					<div class="new-env-form">
						<form {...createEnv}>
							<div class="field">
								<label for="env-slug">Slug</label>
								<input id="env-slug" {...createEnv.fields.slug.as('text')} required pattern="[a-z0-9\-]+" placeholder="staging" />
								{#each createEnv.fields.slug.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
							</div>
							<div class="actions">
								<button type="button" class="btn btn-sm" onclick={() => (showNewEnv = false)}>Cancel</button>
								<button type="submit" class="btn btn-sm btn-primary">Create</button>
							</div>
						</form>
					</div>
				{/if}

				{#key envRefresh}
				{#await getEnvironments() then { envs, deployZone, defaultEnvSlug }}
					{#if envs.length === 0 && !showNewEnv}
						<p class="muted empty-envs">No environments yet.</p>
					{:else}
						<div class="env-list">
							{#each envs as env}
								{@const isDefault = defaultEnvSlug === env.spec.slug}
								<div class="env-card">
									<div class="env-header">
										<div class="env-info">
											<div class="env-title">
												<span class="env-name">{env.spec.slug}</span>
												{#if isDefault}<span class="badge running">default</span>{/if}
											</div>
											{#if env.status?.namespace}
												<button class="ns-copy" onclick={() => copyNs(env.status.namespace)} title="Copy namespace">
													<code class="mono small">{env.status.namespace}</code>
													<span class="copy-hint">{copiedNs === env.status.namespace ? '✓' : '⎘'}</span>
												</button>
											{:else}
												<code class="mono small muted">Provisioning…</code>
											{/if}
											{#if env.status?.subdomain}
												<a class="platform-url" href="https://{env.status.subdomain}.{deployZone}" target="_blank" rel="noopener">
													{env.status.subdomain}.{deployZone} ↗
												</a>
											{/if}
										</div>
										<div class="env-actions">
											<div class="dropdown" class:open={openDropdown === env.metadata.name}>
												<button class="btn btn-sm btn-subtle dropdown-trigger" onclick={() => toggleDropdown(env.metadata.name)} title="Actions">⋯</button>
												<div class="dropdown-menu">
													<button class="dropdown-item" onclick={() => handleSetDefault(isDefault ? null : env.spec.slug)}>
														<span class="check-mark">{isDefault ? '✓' : ''}</span>
														Default
													</button>
													<button class="dropdown-item" onclick={() => { domainEnv = domainEnv === env.metadata.name ? null : env.metadata.name; openDropdown = null; }}>
														Set domain
													</button>
												</div>
											</div>
										</div>
									</div>

									{#if env.status?.domains?.length}
										<div class="domains">
											{#each env.status.domains as domain}
												<div class="domain-row">
													<span>{domain.fqdn}</span>
													<span class="badge {domain.certStatus === 'Issued' ? 'running' : 'pending'}">{domain.certStatus ?? 'Pending'}</span>
													{#if domain.certStatus !== 'Issued'}
														<span class="muted">Point CNAME → gw.enzarb.dev</span>
													{/if}
												</div>
											{/each}
										</div>
									{/if}

									{#if domainEnv === env.metadata.name && domainForm}
										<form {...domainForm} class="domain-form">
											<input {...domainForm.fields.envName.as('hidden', env.metadata.name)} />
											<input {...domainForm.fields.fqdn.as('text')} placeholder="app.yourdomain.com" required />
											<button type="submit" class="btn btn-sm btn-primary">Set</button>
											<button type="button" class="btn btn-sm" onclick={() => (domainEnv = null)}>Cancel</button>
										</form>
									{/if}
								</div>
							{/each}
						</div>
					{/if}
				{:catch err}
					<p class="muted">Could not load environments: {err?.message ?? 'unknown error'}</p>
				{/await}
				{/key}
			</div>
		</div>
	</div>
{:catch}
	<p class="muted">Could not load project.</p>
{/await}

<style>
	.overview { display: flex; flex-direction: column; gap: 1.5rem; }
	.storage-card { display: flex; flex-direction: column; gap: 0.25rem; }
	.card-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--color-text-muted); margin-bottom: 0.375rem; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.small { font-size: 11px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.disk-bar-wrap { height: 4px; background: var(--color-border); border-radius: 2px; margin-top: 0.5rem; overflow: hidden; }
	.disk-bar { height: 100%; background: var(--color-accent); border-radius: 2px; transition: width 0.3s; }
	.disk-bar.disk-warn { background: #e0a020; }
	.disk-label { font-size: 11px; color: var(--color-text-muted); margin-top: 0.25rem; }
	.conditions h3 { margin-bottom: 0.75rem; font-size: 14px; }

	/* Two-column layout for Images + Environments */
	.two-col { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; align-items: start; }
	@media (max-width: 680px) { .two-col { grid-template-columns: 1fr; } }

	/* Images section */
	.empty-msg { margin: 0; line-height: 1.5; }
	.image-list { display: flex; flex-direction: column; gap: 0.375rem; margin-top: 0.25rem; }
	.image-row { display: flex; justify-content: space-between; align-items: center; padding: 0.3rem 0; border-bottom: 1px solid var(--color-border); }
	.image-row:last-child { border-bottom: none; }
	.image-name { font-size: 12px; color: var(--color-text); }
	.image-meta { font-size: 11px; }

	/* Environments section */
	.env-section-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.75rem; }
	.env-section-header .card-label { margin-bottom: 0; }
	.btn-subtle { background: none; border-color: transparent; color: var(--color-text-muted); }
	.btn-subtle:hover { border-color: var(--color-border); color: var(--color-text); }
	.empty-envs { margin: 0; }
	.new-env-form { border-top: 1px solid var(--color-border); padding-top: 0.75rem; margin-bottom: 0.75rem; }
	.field { margin-bottom: 0.75rem; }
	label { display: block; font-weight: 500; font-size: 13px; margin-bottom: 0.25rem; }
	.field-error { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0 0; }
	.actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.env-list { display: flex; flex-direction: column; gap: 0.5rem; }
	.env-card { border-top: 1px solid var(--color-border); padding-top: 0.5rem; display: flex; flex-direction: column; gap: 0.5rem; }
	.env-header { display: flex; justify-content: space-between; align-items: flex-start; }
	.env-info { display: flex; flex-direction: column; gap: 0.15rem; }
	.env-title { display: flex; align-items: center; gap: 0.4rem; }
	.env-name { font-weight: 600; font-size: 13px; }
	.env-actions { display: flex; gap: 0.4rem; align-items: center; flex-shrink: 0; }
	.ns-copy { background: none; border: none; cursor: pointer; padding: 0; display: flex; align-items: center; gap: 0.3rem; }
	.ns-copy:hover .mono { text-decoration: underline; }
	.copy-hint { font-size: 11px; color: var(--color-text-muted); }
	.platform-url { font-family: var(--font-mono); font-size: 12px; color: var(--color-accent); }
	.platform-url:hover { text-decoration: underline; }
	.domains { display: flex; flex-direction: column; gap: 0.4rem; padding-left: 0.5rem; }
	.domain-row { display: flex; align-items: center; gap: 0.6rem; font-size: 13px; }
	.domain-form { display: flex; gap: 0.5rem; align-items: center; }
	.domain-form input[type=text] { max-width: 200px; }

	/* Dropdown */
	.dropdown { position: relative; }
	.dropdown-trigger { padding: 0.25rem 0.5rem; font-size: 14px; line-height: 1; }
	.dropdown-menu {
		display: none;
		position: absolute;
		right: 0;
		top: calc(100% + 4px);
		min-width: 140px;
		background: var(--color-surface);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		box-shadow: var(--shadow);
		z-index: 10;
		overflow: hidden;
	}
	.dropdown.open .dropdown-menu { display: block; }
	.dropdown-item {
		display: flex;
		align-items: center;
		gap: 0.5rem;
		width: 100%;
		padding: 0.45rem 0.75rem;
		background: none;
		border: none;
		color: var(--color-text);
		font-size: 13px;
		cursor: pointer;
		text-align: left;
	}
	.dropdown-item:hover { background: var(--color-surface-2); }
	.check-mark { display: inline-block; width: 1em; text-align: center; font-size: 12px; }
</style>

<script lang="ts">
	import { getEnvironments, createEnv, addDomain, setDefaultEnv } from '$lib/remote/environments.remote';
	import { getProject } from '$lib/remote/projects.remote';
	let showNewEnv = $state(false);
	let domainEnv: string | null = $state(null);
</script>

<div class="deployments-page">
	<div class="page-header">
		<h3>Environments</h3>
		<button class="btn btn-primary" onclick={() => (showNewEnv = !showNewEnv)}>New environment</button>
	</div>

	{#if showNewEnv}
		<div class="card new-env-form">
			<h4>Create environment</h4>
			<form {...createEnv}>
				<div class="field">
					<label for="env-slug">Slug</label>
					<input id="env-slug" {...createEnv.fields.slug.as('text')} required pattern="[a-z0-9-]+" placeholder="staging" />
					{#each createEnv.fields.slug.issues() as issue}<p class="field-error">{issue.message}</p>{/each}
				</div>
				<div class="actions">
					<button type="button" class="btn" onclick={() => (showNewEnv = false)}>Cancel</button>
					<button type="submit" class="btn btn-primary">Create</button>
				</div>
			</form>
		</div>
	{/if}

	{#await Promise.all([getEnvironments(), getProject()]) then [{ envs, deployZone }, project]}
		{@const defaultEnvSlug = project.metadata?.annotations?.['enzarb.io/default-environment'] ?? null}
		<div class="env-list">
			{#each envs as env}
				{@const isDefault = defaultEnvSlug === env.spec.slug}
				<div class="card env-card">
					<div class="env-header">
						<div>
							<div class="env-title">
								<span class="env-name">{env.spec.slug}</span>
								{#if isDefault}<span class="badge running">default</span>{/if}
							</div>
							<code class="mono small">{env.status?.namespace ?? 'Provisioning…'}</code>
							{#if env.status?.subdomain}
								<a class="platform-url" href="https://{env.status.subdomain}.{deployZone}" target="_blank" rel="noopener">
									{env.status.subdomain}.{deployZone}
								</a>
							{/if}
						</div>
						<div class="env-actions">
							{#if !isDefault}
								<button
									class="btn btn-sm"
									onclick={() => setDefaultEnv({ envSlug: env.spec.slug })}
								>Set as default</button>
							{:else}
								<button
									class="btn btn-sm"
									onclick={() => setDefaultEnv({ envSlug: null })}
								>Unset default</button>
							{/if}
							<button class="btn btn-sm" onclick={() => (domainEnv = env.metadata.name)}>Add domain</button>
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

					{#if domainEnv === env.metadata.name}
						{@const domainForm = addDomain.for(env.metadata.name)}
						<form {...domainForm} class="domain-form">
							<input {...domainForm.fields.envName.as('hidden', env.metadata.name)} />
							<input {...domainForm.fields.fqdn.as('text')} placeholder="app.yourdomain.com" required />
							<button type="submit" class="btn btn-primary">Add</button>
							<button type="button" class="btn" onclick={() => (domainEnv = null)}>Cancel</button>
						</form>
					{/if}
				</div>
			{:else}
				<p class="muted">No environments yet.</p>
			{/each}
		</div>
	{:catch err}
		<p class="muted">Could not load environments: {err?.message ?? 'unknown error'}</p>
	{/await}
</div>

<style>
	.deployments-page { display: flex; flex-direction: column; gap: 1rem; }
	.page-header { display: flex; justify-content: space-between; align-items: center; }
	.new-env-form { max-width: 400px; }
	.new-env-form h4 { margin-bottom: 1rem; }
	.field { margin-bottom: 1rem; }
	label { display: block; font-weight: 500; margin-bottom: 0.25rem; }
	.field-error { color: var(--color-danger); font-size: 12px; margin: 0.25rem 0 0; }
	.actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.env-list { display: flex; flex-direction: column; gap: 0.75rem; }
	.env-card { display: flex; flex-direction: column; gap: 0.75rem; }
	.env-header { display: flex; justify-content: space-between; align-items: flex-start; }
	.env-title { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.25rem; }
	.env-name { font-weight: 600; }
	.env-actions { display: flex; gap: 0.5rem; align-items: center; flex-shrink: 0; }
	.mono { font-family: var(--font-mono); }
	.small { font-size: 12px; color: var(--color-text-muted); }
	.domains { display: flex; flex-direction: column; gap: 0.5rem; }
	.domain-row { display: flex; align-items: center; gap: 0.75rem; font-size: 13px; }
	.domain-form { display: flex; gap: 0.5rem; align-items: center; }
	.domain-form input[type=text] { max-width: 280px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.btn-sm { padding: 0.3rem 0.7rem; font-size: 12px; }
	.platform-url { display: block; font-family: var(--font-mono); font-size: 12px; color: var(--color-accent); margin-top: 0.15rem; }
</style>

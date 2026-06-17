<script lang="ts">
	import type { PageData, ActionData } from './$types';
	import { enhance } from '$app/forms';
	let { data, form }: { data: PageData; form: ActionData } = $props();
	let showNewEnv = $state(false);
	let domainEnv: string | null = $state(null);
</script>

<div class="deployments-page">
	<div class="page-header">
		<h3>Environments</h3>
		<button class="btn btn-primary" onclick={() => showNewEnv = !showNewEnv}>New environment</button>
	</div>

	{#if showNewEnv}
		<form method="POST" action="?/createEnv" use:enhance class="card new-env-form">
			<h4>Create environment</h4>
			<div class="field">
				<label for="slug">Slug</label>
				<input id="slug" name="slug" type="text" required pattern="[a-z0-9-]+" placeholder="staging" />
			</div>
			{#if form?.error}<div class="error">{form.error}</div>{/if}
			<div class="actions">
				<button type="button" class="btn" onclick={() => showNewEnv = false}>Cancel</button>
				<button type="submit" class="btn btn-primary">Create</button>
			</div>
		</form>
	{/if}

	<div class="env-list">
		{#each data.envs as env}
			<div class="card env-card">
				<div class="env-header">
					<div>
						<span class="env-name">{env.spec.slug}</span>
						<code class="mono small">{env.status?.namespace ?? 'Provisioning…'}</code>
					</div>
					<button class="btn" onclick={() => domainEnv = env.metadata.name}>Add domain</button>
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
					<form method="POST" action="?/addDomain" use:enhance class="domain-form">
						<input type="hidden" name="envName" value={env.metadata.name} />
						<input name="fqdn" type="text" placeholder="app.yourdomain.com" required />
						<button type="submit" class="btn btn-primary">Add</button>
						<button type="button" class="btn" onclick={() => domainEnv = null}>Cancel</button>
					</form>
				{/if}
			</div>
		{:else}
			<p class="muted">No environments yet.</p>
		{/each}
	</div>
</div>

<style>
	.deployments-page { display: flex; flex-direction: column; gap: 1rem; }
	.page-header { display: flex; justify-content: space-between; align-items: center; }
	.new-env-form { max-width: 400px; }
	.new-env-form h4 { margin-bottom: 1rem; }
	.field { margin-bottom: 1rem; }
	label { display: block; font-weight: 500; margin-bottom: 0.25rem; }
	.actions { display: flex; gap: 0.5rem; justify-content: flex-end; }
	.env-list { display: flex; flex-direction: column; gap: 0.75rem; }
	.env-card { display: flex; flex-direction: column; gap: 0.75rem; }
	.env-header { display: flex; justify-content: space-between; align-items: flex-start; }
	.env-name { font-weight: 600; display: block; margin-bottom: 0.25rem; }
	.mono { font-family: var(--font-mono); }
	.small { font-size: 12px; color: var(--color-text-muted); }
	.domains { display: flex; flex-direction: column; gap: 0.5rem; }
	.domain-row { display: flex; align-items: center; gap: 0.75rem; font-size: 13px; }
	.domain-form { display: flex; gap: 0.5rem; align-items: center; }
	.domain-form input { max-width: 280px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.error { color: var(--color-danger); }
</style>

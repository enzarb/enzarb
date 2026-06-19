<script lang="ts">
	import { getProject } from '$lib/remote/projects.remote';
</script>

{#await getProject() then project}
	<div class="overview">
		<div class="info-grid">
			<div class="card">
				<div class="card-label">Agent URL</div>
				{#if project.status?.agentPath}
					<code class="mono">https://enzarb.dev{project.status.agentPath}</code>
				{:else}
					<span class="muted">Provisioning…</span>
				{/if}
			</div>
			<div class="card">
				<div class="card-label">Service Account</div>
				<code class="mono">{project.status?.serviceAccountName ?? '–'}</code>
			</div>
			<div class="card">
				<div class="card-label">Storage</div>
				<code class="mono">{project.spec.storage?.size ?? '–'}</code>
			</div>
			<div class="card">
				<div class="card-label">Tools</div>
				<div class="tools">
					{#each project.spec.tools ?? [] as tool}
						<span class="badge">{tool.name}@{tool.version ?? 'latest'}</span>
					{:else}
						<span class="muted">None selected</span>
					{/each}
				</div>
			</div>
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
	</div>
{/await}

<style>
	.info-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 1rem; margin-bottom: 1.5rem; }
	.card-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--color-text-muted); margin-bottom: 0.375rem; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.tools { display: flex; flex-wrap: wrap; gap: 0.25rem; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.conditions { margin-top: 1rem; }
	.conditions h3 { margin-bottom: 0.75rem; font-size: 14px; }
</style>

<script lang="ts">
	import { getProject, getAgentToken } from '$lib/remote/projects.remote';

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
</script>

{#await Promise.all([getProject(), getAgentToken()]) then [project, token]}
	<div class="overview">
		<div class="info-grid">
			<div class="card">
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
{:catch}
	<p class="muted">Could not load project.</p>
{/await}

<style>
	.info-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 1rem; margin-bottom: 1.5rem; }
	.card-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--color-text-muted); margin-bottom: 0.375rem; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.tools { display: flex; flex-wrap: wrap; gap: 0.25rem; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
	.disk-bar-wrap { height: 4px; background: var(--color-border); border-radius: 2px; margin-top: 0.5rem; overflow: hidden; }
	.disk-bar { height: 100%; background: var(--color-accent); border-radius: 2px; transition: width 0.3s; }
	.disk-bar.disk-warn { background: #e0a020; }
	.disk-label { font-size: 11px; color: var(--color-text-muted); margin-top: 0.25rem; }
	.conditions { margin-top: 1rem; }
	.conditions h3 { margin-bottom: 0.75rem; font-size: 14px; }
</style>

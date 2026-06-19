<script lang="ts">
	import { getUsageSummary, getUsageByProject, getInvoices } from '$lib/remote/billing.remote';

	const resourceLabels: Record<string, string> = {
		cpu_seconds: 'CPU', mem_gib_seconds: 'Memory',
		net_ingress_bytes: 'Network In', net_egress_bytes: 'Network Out',
		storage_gib_seconds: 'Storage'
	};
</script>

<h2>Billing</h2>

<section class="section">
	<h3>This month's usage</h3>
	<div class="usage-grid">
		{#each await getUsageSummary() as row}
			<div class="card usage-card">
				<div class="usage-label">{resourceLabels[row.resource_type] ?? row.resource_type}</div>
				<div class="usage-value">{Number(row.total).toLocaleString()} <span class="unit">{row.unit}</span></div>
			</div>
		{:else}
			<p class="muted">No usage this month.</p>
		{/each}
	</div>
</section>

<section class="section">
	<h3>Usage by project</h3>
	<table>
		<thead><tr><th>Project</th><th>Resource</th><th>Total</th><th>Unit</th></tr></thead>
		<tbody>
			{#each await getUsageByProject() as row}
				<tr>
					<td><code class="mono">{row.project_id}</code></td>
					<td>{resourceLabels[row.resource_type] ?? row.resource_type}</td>
					<td>{Number(row.total).toLocaleString()}</td>
					<td class="muted">{row.unit}</td>
				</tr>
			{:else}
				<tr><td colspan="4" class="muted">No data</td></tr>
			{/each}
		</tbody>
	</table>
</section>

<section class="section">
	<h3>Invoices</h3>
	<table>
		<thead><tr><th>Period</th><th>Total</th><th>Status</th></tr></thead>
		<tbody>
			{#each await getInvoices() as inv}
				<tr>
					<td>{new Date(inv.period_start).toLocaleDateString()} – {new Date(inv.period_end).toLocaleDateString()}</td>
					<td>${(inv.total_cents / 100).toFixed(2)}</td>
					<td><span class="badge {inv.status === 'paid' ? 'running' : 'pending'}">{inv.status}</span></td>
				</tr>
			{:else}
				<tr><td colspan="3" class="muted">No invoices yet</td></tr>
			{/each}
		</tbody>
	</table>
</section>

<style>
	.section { margin-bottom: 2rem; }
	.section h3 { margin-bottom: 0.75rem; font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
	.usage-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(160px, 1fr)); gap: 0.75rem; }
	.usage-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--color-text-muted); margin-bottom: 0.375rem; }
	.usage-value { font-size: 1.25rem; font-weight: 600; font-variant-numeric: tabular-nums; }
	.unit { font-size: 12px; font-weight: 400; color: var(--color-text-muted); }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

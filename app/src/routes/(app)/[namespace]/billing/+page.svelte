<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { getUsageSummary, getUsageByProject, getInvoices, getEstimatedCost } from '$lib/remote/billing.remote';

	const resourceLabels: Record<string, string> = {
		cpu_seconds: 'CPU', mem_gib_seconds: 'Memory',
		net_ingress_bytes: 'Network In', net_egress_bytes: 'Network Out',
		storage_gib_seconds: 'Storage'
	};

	const usd = (n: number) =>
		n.toLocaleString('en-US', { style: 'currency', currency: 'USD', minimumFractionDigits: 2, maximumFractionDigits: n < 1 ? 4 : 2 });

	// Metering writes usage every ~60s; refresh the live figures so the dashboard
	// tracks spend in near real time without a manual reload.
	let timer: ReturnType<typeof setInterval> | undefined;
	onMount(() => {
		timer = setInterval(() => {
			getEstimatedCost().refresh();
			getUsageSummary().refresh();
			getUsageByProject().refresh();
		}, 15_000);
	});
	onDestroy(() => clearInterval(timer));
</script>

<h2>Billing</h2>

{#await getEstimatedCost() then est}
	<section class="estimate">
		<div class="estimate-head">
			<span class="estimate-label">Estimated cost this month</span>
			<span class="live"><span class="dot"></span>live</span>
		</div>
		<div class="estimate-total">{usd(est.total)}</div>
		<div class="estimate-lines">
			<span>CPU {usd(est.lines.cpu)}</span>
			<span>Memory {usd(est.lines.mem)}</span>
			<span>Net In {usd(est.lines.net_in)}</span>
			<span>Net Out {usd(est.lines.net_out)}</span>
			<span>Storage {usd(est.lines.storage)}</span>
		</div>
		<p class="estimate-note">Running estimate to date. Invoices are issued at month end.</p>
	</section>
{/await}

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
	.estimate { background: var(--color-surface); border: 1px solid var(--color-border); border-radius: 8px; padding: 1.25rem 1.5rem; margin-bottom: 2rem; }
	.estimate-head { display: flex; align-items: center; justify-content: space-between; }
	.estimate-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--color-text-muted); }
	.live { display: inline-flex; align-items: center; gap: 0.35rem; font-size: 11px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
	.dot { width: 7px; height: 7px; border-radius: 50%; background: #3fb950; animation: pulse 2s ease-in-out infinite; }
	@keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }
	.estimate-total { font-size: 2rem; font-weight: 700; font-variant-numeric: tabular-nums; margin: 0.25rem 0 0.5rem; }
	.estimate-lines { display: flex; flex-wrap: wrap; gap: 0.5rem 1.25rem; font-size: 12px; color: var(--color-text-muted); font-variant-numeric: tabular-nums; }
	.estimate-note { font-size: 11px; color: var(--color-text-muted); margin: 0.75rem 0 0; }

	.section { margin-bottom: 2rem; }
	.section h3 { margin-bottom: 0.75rem; font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; }
	.usage-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(160px, 1fr)); gap: 0.75rem; }
	.usage-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; color: var(--color-text-muted); margin-bottom: 0.375rem; }
	.usage-value { font-size: 1.25rem; font-weight: 600; font-variant-numeric: tabular-nums; }
	.unit { font-size: 12px; font-weight: 400; color: var(--color-text-muted); }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

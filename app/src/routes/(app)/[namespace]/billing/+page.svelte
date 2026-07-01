<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import {
		getInvoices,
		getInvoicePdf,
		getEstimatedCost,
		getProjectRollup,
		getCostByComponent,
		getCostTimeSeries,
		getUsageWithLimits,
		getPublicPricing
	} from '$lib/remote/billing.remote';

	let downloadingInvoice: string | null = $state(null);
	async function downloadInvoice(id: string, periodStart: string) {
		downloadingInvoice = id;
		try {
			const bytes = await getInvoicePdf({ invoiceId: id });
			const blob = new Blob([new Uint8Array(bytes)], { type: 'application/pdf' });
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `invoice-${new Date(periodStart).toISOString().slice(0, 7)}.pdf`;
			a.click();
			URL.revokeObjectURL(url);
		} finally {
			downloadingInvoice = null;
		}
	}
	import { RESOURCE_TYPES } from '$lib/billing';
	import StackedBarChart from '$lib/components/StackedBarChart.svelte';

	const resourceLabels: Record<string, string> = {
		vcpu_hours: 'CPU',
		mem_gib_hours: 'Memory',
		net_ingress_internal_bytes: 'Net In (internal)',
		net_egress_internal_bytes: 'Net Out (internal)',
		net_ingress_external_bytes: 'Net In (external)',
		net_egress_external_bytes: 'Net Out (external)',
		block_storage_gib_months: 'Block Storage',
		registry_gib_months: 'Registry Storage'
	};

	const componentLabels: Record<string, string> = {
		workspace: 'Workspaces',
		environment: 'Deploy environments',
		zot: 'Registry'
	};

	// Stable colour per resource type for the chart + legend.
	const resourceColors: Record<string, string> = {
		vcpu_hours: '#58a6ff',
		mem_gib_hours: '#3fb950',
		net_ingress_internal_bytes: '#d29922',
		net_egress_internal_bytes: '#e3b341',
		net_ingress_external_bytes: '#db6d28',
		net_egress_external_bytes: '#f0883e',
		block_storage_gib_months: '#a371f7',
		registry_gib_months: '#56d4dd'
	};

	const usd = (n: number) =>
		n.toLocaleString('en-US', {
			style: 'currency',
			currency: 'USD',
			minimumFractionDigits: 2,
			maximumFractionDigits: n < 1 ? 4 : 2
		});
	const usdRate = (n: number) => '$' + n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 });

	const fmtVCPUHours = (h: number) =>
		h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' vCPU-hr';
	const fmtGiBHours = (h: number) =>
		h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' GiB-hr';
	const fmtGiBMonths = (m: number) =>
		m.toLocaleString('en-US', { maximumFractionDigits: 3 }) + ' GiB-mo';
	const fmtBytes = (bytes: number) => {
		if (bytes === 0) return '0 B';
		const units = ['B', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.min(Math.floor(Math.log2(bytes) / 10), units.length - 1);
		const val = bytes / Math.pow(1024, i);
		return val.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' ' + units[i];
	};

	// Filter state for the cost-over-time chart.
	let days = $state(30);
	let selectedResources = $state<string[]>([]);
	let selectedProjects = $state<string[]>([]);

	const seriesArgs = $derived({
		days,
		projectIds: selectedProjects,
		resourceTypes: selectedResources as (typeof RESOURCE_TYPES)[number][]
	});

	type ChartBucket = { label: string; segments: { key: string; value: number }[] };
	let chartBuckets = $state<ChartBucket[]>([]);
	let chartLoading = $state(false);

	$effect(() => {
		const args = seriesArgs;
		chartLoading = true;
		getCostTimeSeries(args).then((series) => {
			chartBuckets = series.map((s) => ({
				label: s.day,
				segments: Object.entries(s.segments).map(([key, value]) => ({ key, value: value as number }))
			}));
			chartLoading = false;
		});
	});

	function toggle(list: string[], value: string): string[] {
		return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
	}

	// Metering writes usage every ~60s; refresh live figures periodically.
	let timer: ReturnType<typeof setInterval> | undefined;
	onMount(() => {
		timer = setInterval(() => {
			getEstimatedCost().refresh();
			getUsageWithLimits().refresh();
			getProjectRollup().refresh();
			getCostByComponent().refresh();
			getCostTimeSeries(seriesArgs).refresh();
		}, 15_000);
	});
	onDestroy(() => clearInterval(timer));

	function pct(used: number, limit: number) {
		if (limit <= 0) return 0;
		return Math.min(100, (used / limit) * 100);
	}

	// Single call site per query, tied to this component's reactive scope —
	// reading the same remote query from multiple {#await} blocks (e.g. the
	// project list was read separately by the filter chips and the table)
	// created a fresh subscription in each spot and warned "derived_inert" when
	// one of those scopes tore down while the shared cached query lived on.
	const estimatedCost = $derived(getEstimatedCost());
	const usageAndPricing = $derived(Promise.all([getUsageWithLimits(), getPublicPricing()]));
	const costByComponent = $derived(getCostByComponent());
	const projectRollup = $derived(getProjectRollup());
	const invoices = $derived(getInvoices());
</script>

<h2>Billing</h2>

{#await estimatedCost then est}
	<section class="estimate">
		<div class="estimate-head">
			<span class="estimate-label">Estimated cost this month</span>
			<span class="live"><span class="dot"></span>live</span>
		</div>
		<div class="estimate-total">{usd(est.total)}</div>
		<div class="estimate-lines">
			<span>CPU {usd(est.lines.cpu)}</span>
			<span>Memory {usd(est.lines.mem)}</span>
			<span>Net In (int) {usd(est.lines.net_in_internal)}</span>
			<span>Net Out (int) {usd(est.lines.net_out_internal)}</span>
			<span>Net In (ext) {usd(est.lines.net_in_external)}</span>
			<span>Net Out (ext) {usd(est.lines.net_out_external)}</span>
			<span>Storage {usd(est.lines.storage)}</span>
			<span>Registry {usd(est.lines.zot)}</span>
		</div>
		<p class="estimate-note">Running estimate to date. Invoices are issued at month end.</p>
	</section>
{/await}

{#await usageAndPricing then [u, rates]}
<section class="section">
	<h3>Usage this month</h3>
	<div class="usage-meters">
		<div class="meter-row">
			<div class="meter-labels">
				<span>CPU</span>
				<span class="meter-value">{fmtVCPUHours(u.vcpu_hours)} / {fmtVCPUHours(u.free_vcpu_hours)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill" style="width:{pct(u.vcpu_hours, u.free_vcpu_hours)}%"></div></div>
			<div class="meter-rate">{usdRate(rates.vcpuHoursPerUnit)} / vCPU-hr after free tier</div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Memory</span>
				<span class="meter-value">{fmtGiBHours(u.mem_gib_hours)} / {fmtGiBHours(u.free_mem_gib_hours)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill" style="width:{pct(u.mem_gib_hours, u.free_mem_gib_hours)}%"></div></div>
			<div class="meter-rate">{usdRate(rates.memGiBHoursPerUnit)} / GiB-hr after free tier</div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Block Storage</span>
				<span class="meter-value">{fmtGiBMonths(u.block_storage_gib_months)} / {fmtGiBMonths(u.free_block_storage_gib_months)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill meter-fill-storage" style="width:{pct(u.block_storage_gib_months, u.free_block_storage_gib_months)}%"></div></div>
			<div class="meter-rate">{usdRate(rates.blockStorageGiBMonthsPerUnit)} / GiB-mo after free tier</div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Registry Storage</span>
				<span class="meter-value">{fmtGiBMonths(u.registry_gib_months)} / {fmtGiBMonths(u.free_registry_gib_months)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill meter-fill-zot" style="width:{pct(u.registry_gib_months, u.free_registry_gib_months)}%"></div></div>
			<div class="meter-rate">{usdRate(rates.registryGiBMonthsPerUnit)} / GiB-mo after free tier</div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net In (internal)</span>
				<span class="meter-value">{fmtBytes(u.net_ingress_internal_bytes)} / {fmtBytes(u.free_net_ingress_internal_bytes)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill meter-fill-net-in-int" style="width:{pct(u.net_ingress_internal_bytes, u.free_net_ingress_internal_bytes)}%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net Out (internal)</span>
				<span class="meter-value">{fmtBytes(u.net_egress_internal_bytes)} / {fmtBytes(u.free_net_egress_internal_bytes)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill meter-fill-net-out-int" style="width:{pct(u.net_egress_internal_bytes, u.free_net_egress_internal_bytes)}%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net In (external)</span>
				<span class="meter-value">{fmtBytes(u.net_ingress_external_bytes)} / {fmtBytes(u.free_net_ingress_external_bytes)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill meter-fill-net-in-ext" style="width:{pct(u.net_ingress_external_bytes, u.free_net_ingress_external_bytes)}%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net Out (external)</span>
				<span class="meter-value">{fmtBytes(u.net_egress_external_bytes)} / {fmtBytes(u.free_net_egress_external_bytes)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill meter-fill-net-out-ext" style="width:{pct(u.net_egress_external_bytes, u.free_net_egress_external_bytes)}%"></div></div>
			<div class="meter-rate">{usdRate(rates.netEgressExternalPerGib)} / GiB after free tier</div>
		</div>
	</div>
</section>
{/await}

<section class="section">
	<h3>Cost by component</h3>
	<div class="usage-grid">
		{#each await costByComponent as row}
			<div class="card usage-card">
				<div class="usage-label">{componentLabels[row.component] ?? row.component}</div>
				<div class="usage-value">{usd(row.cost)}</div>
			</div>
		{/each}
	</div>
</section>

<section class="section">
	<h3>Cost over time</h3>
	<div class="filters">
		<div class="filter-group">
			<span class="filter-label">Range</span>
			{#each [7, 30, 90] as d}
				<button class="chip {days === d ? 'active' : ''}" onclick={() => (days = d)}>{d}d</button>
			{/each}
		</div>
		<div class="filter-group">
			<span class="filter-label">Metric</span>
			{#each RESOURCE_TYPES as rt}
				<button
					class="chip {selectedResources.includes(rt) ? 'active' : ''}"
					style={selectedResources.includes(rt) ? `border-color:${resourceColors[rt]}` : ''}
					onclick={() => (selectedResources = toggle(selectedResources, rt))}
				>
					{resourceLabels[rt] ?? rt}
				</button>
			{/each}
		</div>
		<div class="filter-group">
			<span class="filter-label">Project</span>
			{#each await projectRollup as row}
				<button
					class="chip {selectedProjects.includes(row.project_id) ? 'active' : ''}"
					onclick={() => (selectedProjects = toggle(selectedProjects, row.project_id))}
				>
					{row.project_id}
				</button>
			{/each}
		</div>
	</div>
	<div class="chart-wrap" class:chart-loading={chartLoading}>
		<StackedBarChart
			buckets={chartBuckets}
			colors={resourceColors}
			labels={resourceLabels}
		/>
	</div>
</section>

<section class="section">
	<h3>Usage by project</h3>
	<div class="table-scroll">
		<table>
			<thead>
				<tr>
					<th>Project</th>
					<th>CPU</th>
					<th>Memory</th>
					<th>Net In (int)</th>
					<th>Net Out (int)</th>
					<th>Net In (ext)</th>
					<th>Net Out (ext)</th>
					<th>Block Storage</th>
					<th>Registry Storage</th>
					<th>Cost</th>
				</tr>
			</thead>
			<tbody>
				{#each await projectRollup as row}
					<tr>
						<td><code class="mono">{row.project_id}</code></td>
						<td>{fmtVCPUHours(row.vcpu_hours)}</td>
						<td>{fmtGiBHours(row.mem_gib_hours)}</td>
						<td>{fmtBytes(row.net_ingress_internal_bytes)}</td>
						<td>{fmtBytes(row.net_egress_internal_bytes)}</td>
						<td>{fmtBytes(row.net_ingress_external_bytes)}</td>
						<td>{fmtBytes(row.net_egress_external_bytes)}</td>
						<td>{fmtGiBMonths(row.block_storage_gib_months)}</td>
						<td>{fmtGiBMonths(row.registry_gib_months)}</td>
						<td class="cost">{usd(row.cost)}</td>
					</tr>
				{:else}
					<tr><td colspan="10" class="muted">No data</td></tr>
				{/each}
			</tbody>
		</table>
	</div>
</section>

<section class="section">
	<h3>Invoices</h3>
	<table>
		<thead><tr><th>Period</th><th>Total</th><th>Status</th><th></th></tr></thead>
		<tbody>
			{#each await invoices as inv}
				<tr>
					<td>{new Date(inv.period_start).toLocaleDateString()} – {new Date(inv.period_end).toLocaleDateString()}</td>
					<td>${(inv.total_cents / 100).toFixed(2)}</td>
					<td><span class="badge {inv.status === 'paid' ? 'running' : 'pending'}">{inv.status}</span></td>
					<td>
						<button
							class="btn btn-sm"
							disabled={downloadingInvoice === inv.id}
							onclick={() => downloadInvoice(inv.id, inv.period_start as unknown as string)}
						>
							{downloadingInvoice === inv.id ? 'Preparing…' : 'Download PDF'}
						</button>
					</td>
				</tr>
			{:else}
				<tr><td colspan="4" class="muted">No invoices yet</td></tr>
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

	.filters { display: flex; flex-direction: column; gap: 0.5rem; margin-bottom: 1rem; }
	.filter-group { display: flex; flex-wrap: wrap; align-items: center; gap: 0.375rem; }
	.filter-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--color-text-muted); width: 56px; }
	.chip { font-size: 12px; padding: 0.2rem 0.6rem; border-radius: 999px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.chip:hover { color: var(--color-text); }
	.chip.active { background: var(--color-surface-2); color: var(--color-text); border-color: var(--color-accent, var(--color-text-muted)); }
	.chart-wrap { transition: opacity 0.2s ease; }
	.chart-wrap.chart-loading { opacity: 0.4; }

	/* Usage meters */
	.usage-meters { display: flex; flex-direction: column; gap: 0.75rem; max-width: 560px; }
	.meter-row { display: flex; flex-direction: column; gap: 0.25rem; }
	.meter-labels { display: flex; justify-content: space-between; font-size: 12px; color: var(--color-text-muted); }
	.meter-value { font-variant-numeric: tabular-nums; }
	.meter-track { height: 6px; background: var(--color-surface-2); border-radius: 3px; overflow: hidden; }
	.meter-fill { height: 100%; background: var(--color-accent, #58a6ff); border-radius: 3px; transition: width 0.3s ease; }
	.meter-rate { font-size: 11px; color: var(--color-text-muted); margin-top: 0.2rem; font-variant-numeric: tabular-nums; }
	.meter-fill-storage { background: #a371f7; }
	.meter-fill-zot { background: #56d4dd; }
	.meter-fill-net-in-int { background: #d29922; }
	.meter-fill-net-out-int { background: #e3b341; }
	.meter-fill-net-in-ext { background: #db6d28; }
	.meter-fill-net-out-ext { background: #f0883e; }

	.table-scroll { overflow-x: auto; }
	.cost { font-variant-numeric: tabular-nums; font-weight: 600; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

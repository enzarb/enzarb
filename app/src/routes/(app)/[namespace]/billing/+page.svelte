<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import {
		getInvoices,
		getEstimatedCost,
		getProjectRollup,
		getCostByComponent,
		getCostTimeSeries,
		getUsageWithLimits
	} from '$lib/remote/billing.remote';
	import { RESOURCE_TYPES } from '$lib/billing';
	import StackedBarChart from '$lib/components/StackedBarChart.svelte';

	const resourceLabels: Record<string, string> = {
		cpu_seconds: 'CPU',
		mem_gib_seconds: 'Memory',
		net_ingress_internal_bytes: 'Net In (internal)',
		net_egress_internal_bytes: 'Net Out (internal)',
		net_ingress_external_bytes: 'Net In (external)',
		net_egress_external_bytes: 'Net Out (external)',
		storage_gib_seconds: 'Storage',
		zot_storage_gib_seconds: 'Registry'
	};

	const componentLabels: Record<string, string> = {
		workspace: 'Workspaces',
		environment: 'Deploy environments',
		zot: 'Registry'
	};

	// Stable colour per resource type for the chart + legend.
	const resourceColors: Record<string, string> = {
		cpu_seconds: '#58a6ff',
		mem_gib_seconds: '#3fb950',
		net_ingress_internal_bytes: '#d29922',
		net_egress_internal_bytes: '#e3b341',
		net_ingress_external_bytes: '#db6d28',
		net_egress_external_bytes: '#f0883e',
		storage_gib_seconds: '#a371f7',
		zot_storage_gib_seconds: '#56d4dd'
	};

	const usd = (n: number) =>
		n.toLocaleString('en-US', {
			style: 'currency',
			currency: 'USD',
			minimumFractionDigits: 2,
			maximumFractionDigits: n < 1 ? 4 : 2
		});

	// Adaptive formatters — pick the largest unit that keeps the value ≥ 1.
	const gibHours = (gibSeconds: number) => {
		const h = gibSeconds / 3600;
		return h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' GiB-hr';
	};
	const cpuHours = (cpuSeconds: number) => {
		const h = cpuSeconds / 3600;
		return h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' hr';
	};
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

{#await getUsageWithLimits() then u}
<section class="section">
	<h3>Usage this month</h3>
	<div class="usage-meters">
		<div class="meter-row">
			<div class="meter-labels">
				<span>CPU</span>
				<span class="meter-value">{cpuHours(u.cpu_seconds)} / {cpuHours(u.free_cpu_seconds)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill" style="width:{pct(u.cpu_seconds, u.free_cpu_seconds)}%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Memory</span>
				<span class="meter-value">{gibHours(u.mem_gib_seconds)} / {gibHours(u.free_mem_gib_seconds)} free</span>
			</div>
			<div class="meter-track"><div class="meter-fill" style="width:{pct(u.mem_gib_seconds, u.free_mem_gib_seconds)}%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Storage</span>
				<span class="meter-value">{gibHours(u.storage_gib_seconds)}</span>
			</div>
			<div class="meter-track meter-track-unbounded"><div class="meter-fill meter-fill-storage" style="width:100%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Registry</span>
				<span class="meter-value">{gibHours(u.zot_storage_gib_seconds)}</span>
			</div>
			<div class="meter-track meter-track-unbounded"><div class="meter-fill meter-fill-zot" style="width:100%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net In (internal)</span>
				<span class="meter-value">{fmtBytes(u.net_ingress_internal_bytes)}</span>
			</div>
			<div class="meter-track meter-track-unbounded"><div class="meter-fill meter-fill-net-in-int" style="width:100%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net Out (internal)</span>
				<span class="meter-value">{fmtBytes(u.net_egress_internal_bytes)}</span>
			</div>
			<div class="meter-track meter-track-unbounded"><div class="meter-fill meter-fill-net-out-int" style="width:100%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net In (external)</span>
				<span class="meter-value">{fmtBytes(u.net_ingress_external_bytes)}</span>
			</div>
			<div class="meter-track meter-track-unbounded"><div class="meter-fill meter-fill-net-in-ext" style="width:100%"></div></div>
		</div>
		<div class="meter-row">
			<div class="meter-labels">
				<span>Net Out (external)</span>
				<span class="meter-value">{fmtBytes(u.net_egress_external_bytes)}</span>
			</div>
			<div class="meter-track meter-track-unbounded"><div class="meter-fill meter-fill-net-out-ext" style="width:100%"></div></div>
		</div>
	</div>
</section>
{/await}

<section class="section">
	<h3>Cost by component</h3>
	<div class="usage-grid">
		{#each await getCostByComponent() as row}
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
			{#each await getProjectRollup() as row}
				<button
					class="chip {selectedProjects.includes(row.project_id) ? 'active' : ''}"
					onclick={() => (selectedProjects = toggle(selectedProjects, row.project_id))}
				>
					{row.project_id}
				</button>
			{/each}
		</div>
	</div>
	{#await getCostTimeSeries(seriesArgs) then series}
		<StackedBarChart
			buckets={series.map((s) => ({
				label: s.day,
				segments: Object.entries(s.segments).map(([key, value]) => ({ key, value }))
			}))}
			colors={resourceColors}
			labels={resourceLabels}
		/>
	{/await}
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
					<th>Storage</th>
					<th>Registry</th>
					<th>Cost</th>
				</tr>
			</thead>
			<tbody>
				{#each await getProjectRollup() as row}
					<tr>
						<td><code class="mono">{row.project_id}</code></td>
						<td>{cpuHours(row.cpu_seconds)}</td>
						<td>{gibHours(row.mem_gib_seconds)}</td>
						<td>{fmtBytes(row.net_ingress_internal_bytes)}</td>
						<td>{fmtBytes(row.net_egress_internal_bytes)}</td>
						<td>{fmtBytes(row.net_ingress_external_bytes)}</td>
						<td>{fmtBytes(row.net_egress_external_bytes)}</td>
						<td>{gibHours(row.storage_gib_seconds)}</td>
						<td>{gibHours(row.zot_storage_gib_seconds)}</td>
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

	.filters { display: flex; flex-direction: column; gap: 0.5rem; margin-bottom: 1rem; }
	.filter-group { display: flex; flex-wrap: wrap; align-items: center; gap: 0.375rem; }
	.filter-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--color-text-muted); width: 56px; }
	.chip { font-size: 12px; padding: 0.2rem 0.6rem; border-radius: 999px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.chip:hover { color: var(--color-text); }
	.chip.active { background: var(--color-surface-2); color: var(--color-text); border-color: var(--color-accent, var(--color-text-muted)); }

	/* Usage meters */
	.usage-meters { display: flex; flex-direction: column; gap: 0.75rem; max-width: 560px; }
	.meter-row { display: flex; flex-direction: column; gap: 0.25rem; }
	.meter-labels { display: flex; justify-content: space-between; font-size: 12px; color: var(--color-text-muted); }
	.meter-value { font-variant-numeric: tabular-nums; }
	.meter-track { height: 6px; background: var(--color-surface-2); border-radius: 3px; overflow: hidden; }
	.meter-fill { height: 100%; background: var(--color-accent, #58a6ff); border-radius: 3px; transition: width 0.3s ease; }
	.meter-track-unbounded { opacity: 0.5; }
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

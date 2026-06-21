<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import {
		getInvoices,
		getEstimatedCost,
		getProjectRollup,
		getCostByComponent,
		getCostTimeSeries,
		RESOURCE_TYPES
	} from '$lib/remote/billing.remote';
	import StackedBarChart from '$lib/components/StackedBarChart.svelte';

	const resourceLabels: Record<string, string> = {
		cpu_seconds: 'CPU',
		mem_gib_seconds: 'Memory',
		net_ingress_bytes: 'Network In',
		net_egress_bytes: 'Network Out',
		storage_gib_seconds: 'Storage',
		gitea_storage_gib_seconds: 'Gitea',
		zot_storage_gib_seconds: 'Registry'
	};

	const componentLabels: Record<string, string> = {
		workspace: 'Workspaces',
		environment: 'Deploy environments',
		gitea: 'Gitea',
		zot: 'Registry'
	};

	// Stable colour per resource type for the chart + legend.
	const resourceColors: Record<string, string> = {
		cpu_seconds: '#58a6ff',
		mem_gib_seconds: '#3fb950',
		net_ingress_bytes: '#d29922',
		net_egress_bytes: '#db6d28',
		storage_gib_seconds: '#a371f7',
		gitea_storage_gib_seconds: '#f778ba',
		zot_storage_gib_seconds: '#56d4dd'
	};

	const usd = (n: number) =>
		n.toLocaleString('en-US', {
			style: 'currency',
			currency: 'USD',
			minimumFractionDigits: 2,
			maximumFractionDigits: n < 1 ? 4 : 2
		});

	// gib-seconds → GiB-hours; bytes → GiB. Keeps the rollup table readable.
	const gibHours = (gibSeconds: number) => (gibSeconds / 3600).toLocaleString('en-US', { maximumFractionDigits: 2 });
	const cpuHours = (cpuSeconds: number) => (cpuSeconds / 3600).toLocaleString('en-US', { maximumFractionDigits: 2 });
	const gib = (bytes: number) => (bytes / 1073741824).toLocaleString('en-US', { maximumFractionDigits: 2 });

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
			getProjectRollup().refresh();
			getCostByComponent().refresh();
			getCostTimeSeries(seriesArgs).refresh();
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
			<span>Gitea {usd(est.lines.gitea)}</span>
			<span>Registry {usd(est.lines.zot)}</span>
		</div>
		<p class="estimate-note">Running estimate to date. Invoices are issued at month end.</p>
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
					<th>CPU (hr)</th>
					<th>Memory (GiB-hr)</th>
					<th>Net In (GiB)</th>
					<th>Net Out (GiB)</th>
					<th>Storage (GiB-hr)</th>
					<th>Gitea (GiB-hr)</th>
					<th>Registry (GiB-hr)</th>
					<th>Cost</th>
				</tr>
			</thead>
			<tbody>
				{#each await getProjectRollup() as row}
					<tr>
						<td><code class="mono">{row.project_id}</code></td>
						<td>{cpuHours(row.cpu_seconds)}</td>
						<td>{gibHours(row.mem_gib_seconds)}</td>
						<td>{gib(row.net_ingress_bytes)}</td>
						<td>{gib(row.net_egress_bytes)}</td>
						<td>{gibHours(row.storage_gib_seconds)}</td>
						<td>{gibHours(row.gitea_storage_gib_seconds)}</td>
						<td>{gibHours(row.zot_storage_gib_seconds)}</td>
						<td class="cost">{usd(row.cost)}</td>
					</tr>
				{:else}
					<tr><td colspan="9" class="muted">No data</td></tr>
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

	.table-scroll { overflow-x: auto; }
	.cost { font-variant-numeric: tabular-nums; font-weight: 600; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

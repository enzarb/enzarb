<script lang="ts">
	import { page } from '$app/state';
	import { getProjectDetail, getProjectCostTimeSeries } from '$lib/remote/billing.remote';
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
	const fmtVCPUHours = (h: number) =>
		h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' vCPU-hr';
	const fmtGiBHours = (h: number) =>
		h.toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' GiB-hr';
	const fmtGiBMonths = (m: number) =>
		m.toLocaleString('en-US', { maximumFractionDigits: 3 }) + ' GiB-mo';
	const fmtBytes = (bytes: number) => {
		if (bytes === 0) return '—';
		const units = ['B', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.min(Math.floor(Math.log2(bytes) / 10), units.length - 1);
		return (bytes / Math.pow(1024, i)).toLocaleString('en-US', { maximumFractionDigits: 2 }) + ' ' + units[i];
	};

	let days = $state(30);
	let groupByOwner = $state(true);

	const projectId = $derived(page.params.project ?? '');
	const detail = $derived(getProjectDetail({ projectId }));
	const seriesArgs = $derived({ projectId, days });
	const series = $derived(getProjectCostTimeSeries(seriesArgs));

	type WorkloadRow = {
		label: string;
		owner: string;
		vcpu_hours: number;
		mem_gib_hours: number;
		net_ingress_internal_bytes: number;
		net_egress_internal_bytes: number;
		net_ingress_external_bytes: number;
		net_egress_external_bytes: number;
		cost: number;
	};

	function groupWorkloads(workloads: WorkloadRow[]): WorkloadRow[] {
		const grouped = new Map<string, WorkloadRow>();
		for (const w of workloads) {
			const key = w.owner || w.label;
			const entry = grouped.get(key) ?? {
				label: key,
				owner: w.owner,
				vcpu_hours: 0,
				mem_gib_hours: 0,
				net_ingress_internal_bytes: 0,
				net_egress_internal_bytes: 0,
				net_ingress_external_bytes: 0,
				net_egress_external_bytes: 0,
				cost: 0
			};
			entry.vcpu_hours += w.vcpu_hours;
			entry.mem_gib_hours += w.mem_gib_hours;
			entry.net_ingress_internal_bytes += w.net_ingress_internal_bytes;
			entry.net_egress_internal_bytes += w.net_egress_internal_bytes;
			entry.net_ingress_external_bytes += w.net_ingress_external_bytes;
			entry.net_egress_external_bytes += w.net_egress_external_bytes;
			entry.cost += w.cost;
			grouped.set(key, entry);
		}
		return [...grouped.values()];
	}
</script>

<h2>Billing — {page.params.project}</h2>
<p class="note">Usage this calendar month, broken down by workload, volume, and image.</p>

<section class="section">
	<div class="section-head">
		<h3>Cost over time</h3>
		<div class="filter-group">
			{#each [7, 30, 90] as d}
				<button class="chip {days === d ? 'active' : ''}" onclick={() => (days = d)}>{d}d</button>
			{/each}
		</div>
	</div>
	{#await series then s}
		<StackedBarChart
			buckets={s.map((b) => ({
				label: b.day,
				segments: Object.entries(b.segments).map(([key, value]) => ({ key, value }))
			}))}
			colors={resourceColors}
			labels={resourceLabels}
		/>
	{/await}
</section>

{#await detail then d}

<section class="section">
	<div class="section-head">
		<h3>Workloads</h3>
		<label class="toggle">
			<input type="checkbox" bind:checked={groupByOwner} />
			Group by owner
		</label>
	</div>
	{#if d.workloads.length === 0}
		<p class="muted">No compute usage recorded this month.</p>
	{:else}
		{@const rows = groupByOwner ? groupWorkloads(d.workloads) : d.workloads}
		<div class="table-scroll">
			<table>
				<thead>
					<tr>
						<th>{groupByOwner ? 'Owner' : 'Pod'}</th>
						<th>CPU</th>
						<th>Memory</th>
						<th>Net In (int)</th>
						<th>Net Out (int)</th>
						<th>Net In (ext)</th>
						<th>Net Out (ext)</th>
						<th>Cost</th>
					</tr>
				</thead>
				<tbody>
					{#each rows as w}
						<tr>
							<td><code class="mono">{w.label}</code></td>
							<td>{fmtVCPUHours(w.vcpu_hours)}</td>
							<td>{fmtGiBHours(w.mem_gib_hours)}</td>
							<td>{fmtBytes(w.net_ingress_internal_bytes)}</td>
							<td>{fmtBytes(w.net_egress_internal_bytes)}</td>
							<td>{fmtBytes(w.net_ingress_external_bytes)}</td>
							<td>{fmtBytes(w.net_egress_external_bytes)}</td>
							<td class="cost">{usd(w.cost)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<section class="section">
	<h3>Block Storage</h3>
	{#if d.storage.length === 0}
		<p class="muted">No block storage volumes recorded this month.</p>
	{:else}
		<div class="table-scroll">
			<table>
				<thead>
					<tr>
						<th>Volume</th>
						<th>Usage</th>
						<th>Cost</th>
					</tr>
				</thead>
				<tbody>
					{#each d.storage as v}
						<tr>
							<td><code class="mono">{v.label}</code></td>
							<td>{fmtGiBMonths(v.gib_months)}</td>
							<td class="cost">{usd(v.cost)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<section class="section">
	<h3>Registry Images</h3>
	{#if d.images.length === 0}
		<p class="muted">No registry images recorded this month.</p>
	{:else}
		<div class="table-scroll">
			<table>
				<thead>
					<tr>
						<th>Image</th>
						<th>Usage</th>
						<th>Cost</th>
					</tr>
				</thead>
				<tbody>
					{#each d.images as img}
						<tr>
							<td><code class="mono">{img.label}</code></td>
							<td>{fmtGiBMonths(img.gib_months)}</td>
							<td class="cost">{usd(img.cost)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

{:catch err}
	<p class="muted">Failed to load billing detail: {err.message}</p>
{/await}

<style>
	h2 { margin-bottom: 0.25rem; }
	.note { font-size: 13px; color: var(--color-text-muted); margin-bottom: 2rem; }
	.section { margin-bottom: 2rem; }
	.section-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.75rem; }
	.section-head h3 { margin: 0; }
	.section h3 { font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 0.75rem; }
	.filter-group { display: flex; align-items: center; gap: 0.375rem; }
	.chip { font-size: 12px; padding: 0.2rem 0.6rem; border-radius: 999px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.chip:hover { color: var(--color-text); }
	.chip.active { background: var(--color-surface-2); color: var(--color-text); border-color: var(--color-accent, var(--color-text-muted)); }
	.toggle { display: flex; align-items: center; gap: 0.4rem; font-size: 12px; color: var(--color-text-muted); cursor: pointer; user-select: none; }
	.toggle input { cursor: pointer; }
	.table-scroll { overflow-x: auto; }
	table { width: 100%; border-collapse: collapse; font-size: 13px; }
	th { text-align: left; padding: 0.4rem 0.75rem; font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--color-text-muted); border-bottom: 1px solid var(--color-border); }
	td { padding: 0.5rem 0.75rem; border-bottom: 1px solid var(--color-border); }
	.cost { font-variant-numeric: tabular-nums; font-weight: 600; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

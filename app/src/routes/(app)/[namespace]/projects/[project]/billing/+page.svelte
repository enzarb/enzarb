<script lang="ts">
	import { page } from '$app/state';
	import { getProjectDetail } from '$lib/remote/billing.remote';

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

	const projectId = $derived(page.params.project ?? '');
	const detail = $derived(getProjectDetail({ projectId }));
</script>

<h2>Billing — {page.params.project}</h2>
<p class="note">Usage this calendar month, broken down by workload, volume, and image.</p>

{#await detail then d}

<section class="section">
	<h3>Workloads</h3>
	{#if d.workloads.length === 0}
		<p class="muted">No compute usage recorded this month.</p>
	{:else}
		<div class="table-scroll">
			<table>
				<thead>
					<tr>
						<th>Pod</th>
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
					{#each d.workloads as w}
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
	.section h3 { font-size: 14px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; margin-bottom: 0.75rem; }
	.table-scroll { overflow-x: auto; }
	table { width: 100%; border-collapse: collapse; font-size: 13px; }
	th { text-align: left; padding: 0.4rem 0.75rem; font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--color-text-muted); border-bottom: 1px solid var(--color-border); }
	td { padding: 0.5rem 0.75rem; border-bottom: 1px solid var(--color-border); }
	.cost { font-variant-numeric: tabular-nums; font-weight: 600; }
	.mono { font-family: var(--font-mono); font-size: 12px; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

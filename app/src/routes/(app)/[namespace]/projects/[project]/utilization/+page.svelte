<script lang="ts">
	import { page } from '$app/state';
	import { getProjectUtilization } from '$lib/remote/utilization.remote';
	import { RESOURCE_TYPES, RESOURCE_LABELS, RESOURCE_COLORS, fmtRaw } from '$lib/billing';
	import LineChart from '$lib/components/LineChart.svelte';
	import { toErrorMessage } from '$lib/errors';

	type Row = { minute: string | Date; resource_type: string; label: string | null; total: number };

	let minutes = $state(60);
	let mode = $state<'aggregate' | 'pod'>('aggregate');
	let metric = $state<string>('vcpu_hours');

	let rows = $state<Row[]>([]);
	let loading = $state(true);
	let error = $state('');

	const projectId = $derived(page.params.project ?? '');

	async function refresh() {
		try {
			const data = await getProjectUtilization({ projectId, minutes });
			rows = data as Row[];
			error = '';
		} catch (e) {
			error = toErrorMessage(e, 'Failed to load utilization data');
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		// Access reactive deps so the effect reruns when they change.
		void projectId;
		void minutes;
		loading = true;
		refresh();
		const timer = setInterval(refresh, 60000);
		return () => clearInterval(timer);
	});

	// Build a zero-filled, sorted list of minute buckets (ISO strings) spanning
	// the selected window, so every chart aligns on the same x-axis regardless
	// of gaps in the raw data.
	function minuteBuckets(): string[] {
		const now = new Date();
		now.setSeconds(0, 0);
		const start = new Date(now.getTime() - (minutes - 1) * 60000);
		return Array.from({ length: minutes }, (_, i) =>
			new Date(start.getTime() + i * 60000).toISOString().slice(0, 16)
		);
	}

	function keyOf(r: Row) {
		return new Date(r.minute).toISOString().slice(0, 16);
	}

	const buckets = $derived(minuteBuckets());
	const xLabels = $derived(buckets.map((b) => b.slice(11)));

	// One summed line per resource type, aggregated across all pods.
	const aggregateSeries = $derived.by(() => {
		const byType = new Map<string, Map<string, number>>();
		for (const r of rows) {
			const m = byType.get(r.resource_type) ?? new Map<string, number>();
			m.set(keyOf(r), (m.get(keyOf(r)) ?? 0) + Number(r.total));
			byType.set(r.resource_type, m);
		}
		const out: Record<string, number[]> = {};
		for (const rt of RESOURCE_TYPES) {
			const m = byType.get(rt) ?? new Map<string, number>();
			out[rt] = buckets.map((b) => m.get(b) ?? 0);
		}
		return out;
	});

	const podPalette = ['#58a6ff', '#3fb950', '#d29922', '#db6d28', '#a371f7', '#56d4dd', '#f778ba', '#e3b341'];

	// Per-pod lines for the currently selected metric.
	const podSeries = $derived.by(() => {
		const byLabel = new Map<string, Map<string, number>>();
		for (const r of rows) {
			if (r.resource_type !== metric) continue;
			const lbl = r.label ?? '(unlabeled)';
			const m = byLabel.get(lbl) ?? new Map<string, number>();
			m.set(keyOf(r), (m.get(keyOf(r)) ?? 0) + Number(r.total));
			byLabel.set(lbl, m);
		}
		const labels = [...byLabel.keys()].sort();
		return labels.map((lbl, i) => ({
			key: lbl,
			label: lbl,
			color: podPalette[i % podPalette.length],
			points: buckets.map((b) => byLabel.get(lbl)?.get(b) ?? 0)
		}));
	});
</script>

<h2>Utilization — {page.params.project}</h2>
<p class="note">Raw metering values, sampled every minute. Not a percentage — each chart is scaled to its own metric.</p>

<div class="controls">
	<div class="filter-group">
		{#each [15, 60, 180] as m}
			<button class="chip {minutes === m ? 'active' : ''}" onclick={() => (minutes = m)}>{m}m</button>
		{/each}
	</div>
	<div class="filter-group">
		<button class="chip {mode === 'aggregate' ? 'active' : ''}" onclick={() => (mode = 'aggregate')}>Aggregate</button>
		<button class="chip {mode === 'pod' ? 'active' : ''}" onclick={() => (mode = 'pod')}>Per pod</button>
	</div>
	{#if mode === 'pod'}
		<select class="metric-select" bind:value={metric}>
			{#each RESOURCE_TYPES as rt}
				<option value={rt}>{RESOURCE_LABELS[rt]}</option>
			{/each}
		</select>
	{/if}
</div>

{#if error}
	<p class="muted">Failed to load utilization: {error}</p>
{:else if loading}
	<p class="muted">Loading…</p>
{:else if mode === 'aggregate'}
	<div class="grid">
		{#each RESOURCE_TYPES as rt}
			<div class="panel">
				<h3>{RESOURCE_LABELS[rt]}</h3>
				<LineChart
					series={[{ key: rt, color: RESOURCE_COLORS[rt], points: aggregateSeries[rt] }]}
					{xLabels}
					valueFormatter={(n) => fmtRaw(rt, n)}
				/>
			</div>
		{/each}
	</div>
{:else}
	<div class="panel panel-wide">
		<h3>{RESOURCE_LABELS[metric]} by pod</h3>
		<LineChart series={podSeries} {xLabels} valueFormatter={(n) => fmtRaw(metric, n)} />
	</div>
{/if}

<style>
	h2 { margin-bottom: 0.25rem; }
	.note { font-size: 13px; color: var(--color-text-muted); margin-bottom: 1.5rem; }
	.controls { display: flex; align-items: center; gap: 1rem; margin-bottom: 1.5rem; flex-wrap: wrap; }
	.filter-group { display: flex; align-items: center; gap: 0.375rem; }
	.chip { font-size: 12px; padding: 0.2rem 0.6rem; border-radius: 999px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text-muted); cursor: pointer; }
	.chip:hover { color: var(--color-text); }
	.chip.active { background: var(--color-surface-2); color: var(--color-text); border-color: var(--color-accent, var(--color-text-muted)); }
	.metric-select { font-size: 12px; padding: 0.25rem 0.5rem; border-radius: 4px; border: 1px solid var(--color-border); background: var(--color-surface); color: var(--color-text); }
	.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 1.5rem; }
	.panel { background: var(--color-surface); border: 1px solid var(--color-border); border-radius: 6px; padding: 0.75rem 1rem 0.5rem; }
	.panel-wide { max-width: 640px; }
	.panel h3 { font-size: 12px; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.06em; margin: 0 0 0.5rem; }
	.muted { color: var(--color-text-muted); font-size: 13px; }
</style>

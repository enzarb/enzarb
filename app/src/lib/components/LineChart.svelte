<script lang="ts">
	// Dependency-free SVG multi-line chart. Each series renders one polyline on
	// a shared x-axis (index-based) with a y-axis auto-scaled to the data. One
	// instance = one metric/scale — don't mix units across series.
	interface Series {
		key: string;
		color: string;
		label?: string;
		points: number[]; // y-values, one per x position (index-aligned across series)
	}

	let {
		series,
		xLabels = [],
		valueFormatter = (n: number) => n.toLocaleString('en-US', { maximumFractionDigits: 3 })
	}: {
		series: Series[];
		xLabels?: string[];
		valueFormatter?: (n: number) => string;
	} = $props();

	const W = 360;
	const H = 120;
	const pad = { top: 10, right: 8, bottom: 20, left: 44 };
	const plotW = W - pad.left - pad.right;
	const plotH = H - pad.top - pad.bottom;

	const pointCount = $derived(Math.max(0, ...series.map((s) => s.points.length)));
	const allValues = $derived(series.flatMap((s) => s.points));
	const max = $derived(Math.max(1e-9, ...allValues));
	const min = $derived(Math.min(0, ...allValues));
	const range = $derived(Math.max(1e-9, max - min));

	function xAt(i: number) {
		return pointCount <= 1 ? pad.left : pad.left + (i / (pointCount - 1)) * plotW;
	}
	function yAt(v: number) {
		return pad.top + plotH - ((v - min) / range) * plotH;
	}

	function pathFor(points: number[]) {
		return points.map((v, i) => `${i === 0 ? 'M' : 'L'}${xAt(i).toFixed(1)},${yAt(v).toFixed(1)}`).join(' ');
	}

	const ticks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => ({ f, value: min + f * range })));
	const hasData = $derived(pointCount > 0 && allValues.some((v) => v > 0));
</script>

<svg viewBox="0 0 {W} {H}" class="chart" role="img" aria-label="Utilization over time">
	{#each ticks as t}
		<line
			x1={pad.left}
			x2={W - pad.right}
			y1={pad.top + plotH - t.f * plotH}
			y2={pad.top + plotH - t.f * plotH}
			class="grid"
		/>
		<text x={pad.left - 4} y={pad.top + plotH - t.f * plotH + 3} class="ytick">{valueFormatter(t.value)}</text>
	{/each}

	{#if hasData}
		{#each series as s}
			<path d={pathFor(s.points)} fill="none" stroke={s.color} stroke-width="1.5" />
		{/each}
	{/if}

	{#if xLabels.length > 1}
		<text x={pad.left} y={H - 6} class="xtick" text-anchor="start">{xLabels[0]}</text>
		<text x={W - pad.right} y={H - 6} class="xtick" text-anchor="end">{xLabels[xLabels.length - 1]}</text>
	{/if}
</svg>

{#if !hasData}
	<p class="muted">No data yet.</p>
{/if}

{#if series.length > 1}
	<div class="legend">
		{#each series as s}
			<span class="legend-item">
				<span class="swatch" style="background:{s.color}"></span>
				{s.label ?? s.key}
			</span>
		{/each}
	</div>
{/if}

<style>
	.chart {
		width: 100%;
		max-width: 360px;
		height: auto;
		display: block;
	}
	.grid {
		stroke: var(--color-border);
		stroke-width: 1;
		opacity: 0.5;
	}
	.ytick {
		fill: var(--color-text-muted);
		font-size: 8px;
		text-anchor: end;
		font-variant-numeric: tabular-nums;
	}
	.xtick {
		fill: var(--color-text-muted);
		font-size: 8px;
	}
	.muted {
		color: var(--color-text-muted);
		font-size: 12px;
		margin: 0.25rem 0 0;
	}
	.legend {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem 0.75rem;
		margin-top: 0.5rem;
		font-size: 11px;
		color: var(--color-text-muted);
	}
	.legend-item {
		display: inline-flex;
		align-items: center;
		gap: 0.3rem;
	}
	.swatch {
		width: 8px;
		height: 8px;
		border-radius: 2px;
		display: inline-block;
	}
</style>

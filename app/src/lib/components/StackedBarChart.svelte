<script lang="ts">
	// Dependency-free SVG stacked bar chart. Each bucket renders one bar; each
	// segment within a bar is stacked and coloured by its key. A legend maps keys
	// to colours/labels. Values are assumed to be in dollars and formatted as USD.
	interface Segment {
		key: string;
		value: number;
	}
	interface Bucket {
		label: string;
		segments: Segment[];
	}

	let {
		buckets,
		colors,
		labels = {}
	}: {
		buckets: Bucket[];
		colors: Record<string, string>;
		labels?: Record<string, string>;
	} = $props();

	// Geometry. The SVG uses a fixed viewBox and scales responsively.
	const W = 720;
	const H = 180;
	const pad = { top: 12, right: 12, bottom: 28, left: 48 };
	const plotW = W - pad.left - pad.right;
	const plotH = H - pad.top - pad.bottom;

	const usd = (n: number) =>
		n.toLocaleString('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 2 });

	const totals = $derived(buckets.map((b) => b.segments.reduce((a, s) => a + s.value, 0)));
	const max = $derived(Math.max(1e-9, ...totals));
	const barW = $derived(buckets.length ? (plotW / buckets.length) * 0.7 : 0);
	const step = $derived(buckets.length ? plotW / buckets.length : 0);

	// Keys present across all buckets, for the legend.
	const keys = $derived([...new Set(buckets.flatMap((b) => b.segments.map((s) => s.key)))]);

	function segLayout(bucket: Bucket) {
		let acc = 0;
		return bucket.segments
			.filter((s) => s.value > 0)
			.map((s) => {
				const h = (s.value / max) * plotH;
				const y = pad.top + plotH - acc - h;
				acc += h;
				return { ...s, y, h };
			});
	}

	// 4 horizontal gridlines / y-axis ticks.
	const ticks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => ({ f, value: f * max })));
</script>

{#if buckets.length === 0}
	<p class="muted">No data for the selected range.</p>
{:else}
	<svg viewBox="0 0 {W} {H}" class="chart" role="img" aria-label="Cost over time">
		{#each ticks as t}
			<line
				x1={pad.left}
				x2={W - pad.right}
				y1={pad.top + plotH - t.f * plotH}
				y2={pad.top + plotH - t.f * plotH}
				class="grid"
			/>
			<text x={pad.left - 6} y={pad.top + plotH - t.f * plotH + 3} class="ytick">{usd(t.value)}</text>
		{/each}

		{#each buckets as bucket, i}
			{@const x = pad.left + i * step + (step - barW) / 2}
			{#each segLayout(bucket) as seg}
				<rect
					{x}
					y={seg.y}
					width={barW}
					height={seg.h}
					fill={colors[seg.key] ?? '#888'}
				>
					<title>{bucket.label} · {labels[seg.key] ?? seg.key}: {usd(seg.value)}</title>
				</rect>
			{/each}
			{#if buckets.length <= 31 && i % Math.ceil(buckets.length / 12) === 0}
				<text x={x + barW / 2} y={H - 10} class="xtick">{bucket.label.slice(5)}</text>
			{/if}
		{/each}
	</svg>

	<div class="legend">
		{#each keys as key}
			<span class="legend-item">
				<span class="swatch" style="background:{colors[key] ?? '#888'}"></span>
				{labels[key] ?? key}
			</span>
		{/each}
	</div>
{/if}

<style>
	.chart {
		width: 100%;
		/* Cap the rendered size so the 3:1 SVG can't balloon in height on wide
		   layouts; it still scales down responsively on narrow screens. */
		max-width: 720px;
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
		font-size: 9px;
		text-anchor: end;
		font-variant-numeric: tabular-nums;
	}
	.xtick {
		fill: var(--color-text-muted);
		font-size: 9px;
		text-anchor: middle;
	}
	.legend {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem 1rem;
		margin-top: 0.75rem;
		font-size: 12px;
		color: var(--color-text-muted);
	}
	.legend-item {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
	}
	.swatch {
		width: 10px;
		height: 10px;
		border-radius: 2px;
		display: inline-block;
	}
	.muted {
		color: var(--color-text-muted);
		font-size: 13px;
	}
</style>

<script lang="ts" module>
	export interface Bar {
		label: string;
		/** Null = not reported: no bar, never a zero-height one. */
		value: number | null;
	}
</script>

<script lang="ts">
	/**
	 * Inline-SVG bar chart (no library). A not-reported value leaves a gap and reads as a dash in
	 * the text table, which screen readers get in place of the picture.
	 */
	let {
		bars,
		title,
		format
	}: { bars: Bar[]; title: string; format: (value: number | null) => string } = $props();

	const W = 600;
	const H = 120;
	const max = $derived(Math.max(0, ...bars.map((b) => b.value ?? 0)));
	const step = $derived(W / Math.max(bars.length, 1));
	const height = (v: number) => (max > 0 ? Math.max(1, (v / max) * H) : 1);
</script>

<figure class="ez-barchart">
	<svg viewBox="0 0 {W} {H}" role="img" aria-label={title} preserveAspectRatio="none">
		{#each bars as b, i (i)}
			{#if b.value !== null}
				<rect
					class="ez-barchart__bar"
					x={i * step + step * 0.1}
					y={H - height(b.value)}
					width={step * 0.8}
					height={height(b.value)}
					data-testid="bar"><title>{b.label}: {format(b.value)}</title></rect
				>
			{/if}
		{/each}
	</svg>
	<table class="ez-sr-only">
		<caption>{title}</caption>
		<tbody>
			{#each bars as b, i (i)}
				<tr><th scope="row">{b.label}</th><td>{format(b.value)}</td></tr>
			{/each}
		</tbody>
	</table>
</figure>

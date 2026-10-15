<script lang="ts" module>
	export interface SeriesPoint {
		t: number;
		/** Null = not reported: the line breaks, never dips to zero. */
		v: number | null;
	}

	export interface Line {
		label: string;
		points: SeriesPoint[];
		/** Reference lines (a limit) are dashed and get no area. */
		reference?: boolean;
	}
</script>

<script lang="ts">
	/**
	 * Inline-SVG line and area chart (no library), laid out like `BarChart`. Time runs from `from` to
	 * `to`; buckets further apart than 1.5 steps, or not reported, leave a gap. The text table gives
	 * screen readers the values in place of the picture.
	 */
	let {
		lines,
		title,
		format,
		from,
		to,
		stepMs
	}: {
		lines: Line[];
		title: string;
		format: (value: number | null) => string;
		from: number;
		to: number;
		stepMs: number;
	} = $props();

	const W = 600;
	const H = 120;
	const span = $derived(Math.max(1, to - from));
	const max = $derived(Math.max(0, ...lines.flatMap((l) => l.points.map((p) => p.v ?? 0))));
	const x = (t: number) => ((t - from) / span) * W;
	const y = (v: number) => (max > 0 ? H - (v / max) * (H - 2) - 1 : H - 1);

	/** Runs of consecutive reported points; a missing bucket or a null value ends a run. */
	function segments(points: SeriesPoint[]): { t: number; v: number }[][] {
		const out: { t: number; v: number }[][] = [];
		let prev: number | null = null;
		for (const p of points) {
			if (p.v === null) {
				prev = null;
				continue;
			}
			if (prev === null || p.t - prev > stepMs * 1.5) out.push([]);
			out[out.length - 1].push({ t: p.t, v: p.v });
			prev = p.t;
		}
		return out;
	}

	const line = (s: { t: number; v: number }[]) =>
		s.map((p, i) => `${i === 0 ? 'M' : 'L'}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`).join(' ');
	const area = (s: { t: number; v: number }[]) =>
		`${line(s)} L${x(s[s.length - 1].t).toFixed(1)},${H} L${x(s[0].t).toFixed(1)},${H} Z`;

	const times = $derived([...new Set(lines.flatMap((l) => l.points.map((p) => p.t)))].sort());
	const valueAt = (l: Line, t: number) => l.points.find((p) => p.t === t)?.v ?? null;
</script>

<figure class="ez-serieschart">
	<figcaption class="ez-serieschart__title">{title}</figcaption>
	<svg viewBox="0 0 {W} {H}" role="img" aria-label={title} preserveAspectRatio="none">
		{#each lines as l, li (li)}
			{#each segments(l.points) as s, si (si)}
				{#if s.length === 1}
					<circle
						class="ez-serieschart__dot ez-serieschart__dot--{li}"
						cx={x(s[0].t)}
						cy={y(s[0].v)}
						r="2"
						data-testid="dot"
					/>
				{:else}
					{#if !l.reference && li === 0}
						<path class="ez-serieschart__area" d={area(s)} data-testid="area" />
					{/if}
					<path
						class="ez-serieschart__line ez-serieschart__line--{li}"
						class:ez-serieschart__line--ref={l.reference}
						d={line(s)}
						data-testid="line"
					/>
				{/if}
			{/each}
		{/each}
	</svg>
	{#if lines.length > 1}
		<ul class="ez-serieschart__legend">
			{#each lines as l, li (li)}
				<li><span class="ez-serieschart__key ez-serieschart__key--{li}"></span>{l.label}</li>
			{/each}
		</ul>
	{/if}
	<table class="ez-sr-only">
		<caption>{title}</caption>
		<thead>
			<tr>
				<th scope="col">Time</th>
				{#each lines as l, li (li)}<th scope="col">{l.label}</th>{/each}
			</tr>
		</thead>
		<tbody>
			{#each times as t (t)}
				<tr>
					<th scope="row">{new Date(t).toISOString()}</th>
					{#each lines as l, li (li)}<td>{format(valueAt(l, t))}</td>{/each}
				</tr>
			{/each}
		</tbody>
	</table>
</figure>

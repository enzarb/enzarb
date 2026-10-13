<script lang="ts">
	/**
	 * Placeholder for a list that is loading: `rows` shimmering lines inside a labelled status
	 * region, so assistive tech hears "Loading ..." once. Not a replacement for step lists:
	 * long operations show their steps.
	 */
	let { rows = 3, label = 'Loading' }: { rows?: number; label?: string } = $props();
</script>

<div class="ez-skeleton-rows" role="status" aria-live="polite">
	<span class="ez-sr-only">{label}…</span>
	{#each { length: rows }, i (i)}
		<span
			class="ez-skeleton"
			class:ez-skeleton--short={i === rows - 1 && rows > 1}
			aria-hidden="true"
		></span>
	{/each}
</div>

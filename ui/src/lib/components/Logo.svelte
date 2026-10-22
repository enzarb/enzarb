<script lang="ts">
	// The overlaid EZ mark (E, Z, and their overlap as three plain paths from scripts/build-brand.mjs)
	// plus the name. `mark` sets the name in Jost 700 text; `wordmark` inlines the outlined ENZARB.
	// Fills come from --ez-logo-* tokens so they follow the theme. No ids, clip paths or filters,
	// so any number of instances can share a page.
	import {
		MARK_E,
		MARK_OVERLAP,
		MARK_VIEWBOX,
		MARK_Z,
		WORD_INK,
		WORD_VIEWBOX,
		WORD_Z
	} from '#lib/brand.generated.ts';

	let { variant = 'mark', size = 22 }: { variant?: 'mark' | 'wordmark'; size?: number } = $props();

	const [, , wordW, wordH] = WORD_VIEWBOX.split(' ').map(Number);
	const wordHeight = $derived(Math.round(size * 0.6));
</script>

<a class="ez-logo" href="/" aria-label="enzarb home">
	<svg class="ez-logo__mark" width={size} height={size} viewBox={MARK_VIEWBOX} aria-hidden="true">
		<path fill="var(--ez-logo-e)" d={MARK_E} />
		<path fill="var(--ez-logo-z)" d={MARK_Z} />
		<path fill="var(--ez-logo-overlap)" d={MARK_OVERLAP} />
	</svg>
	{#if variant === 'wordmark'}
		<svg
			class="ez-logo__word"
			height={wordHeight}
			width={Math.round((wordHeight * wordW) / wordH)}
			viewBox={WORD_VIEWBOX}
			aria-hidden="true"
		>
			<path fill="var(--ez-logo-ink)" d={WORD_INK} />
			<path fill="var(--ez-logo-z)" d={WORD_Z} />
		</svg>
	{:else}
		<span class="ez-logo__name" aria-hidden="true">EN<span class="ez-logo__z">Z</span>ARB</span>
	{/if}
</a>

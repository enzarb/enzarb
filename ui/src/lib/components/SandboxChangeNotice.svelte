<script lang="ts">
	import { POD_SHAPE_PRESAVE_COPY } from '#lib/pending-change.ts';
	import Notice from './ui/Notice.svelte';

	/** Pre-save notice; only rendered while an unsaved edit would trigger a drain-first change. */
	let { visible }: { visible: boolean } = $props();
</script>

{#if visible}<Notice tone="info">{POD_SHAPE_PRESAVE_COPY}</Notice>{/if}

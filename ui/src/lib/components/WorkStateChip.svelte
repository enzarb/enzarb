<script lang="ts">
	import Badge from './ui/Badge.svelte';
	import { workStatus } from './ui/status.ts';

	/** A work item's state as a Badge (tone from the work-items state classification). */
	let { state }: { state: string } = $props();
	const s = $derived(workStatus(state));
</script>

<Badge tone={s.tone} hollow={s.hollow}>{state}</Badge>

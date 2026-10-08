<script lang="ts">
	import { stateTone } from '#lib/work-items.ts';

	let { state }: { state: string } = $props();
</script>

<span class="badge chip {stateTone(state)}">{state}</span>

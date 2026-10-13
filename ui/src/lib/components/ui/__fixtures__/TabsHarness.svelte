<script lang="ts">
	import Tabs from '../Tabs.svelte';
</script>

<Tabs
	label="Sections"
	tabs={[
		{ value: 'a', label: 'Alpha' },
		{ value: 'b', label: 'Beta' },
		{ value: 'c', label: 'Gamma' }
	]}
>
	{#snippet panel(v)}<p>Panel {v}</p>{/snippet}
</Tabs>

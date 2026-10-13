<script lang="ts">
	// Test-only: two checkboxes sharing one array name, as `fields.tags.as('checkbox', value)` makes them.
	import Checkbox from '../Checkbox.svelte';

	let { name }: { name: string } = $props();
	let events = $state<string[]>([]);
	let posted = $state('');
</script>

<form
	oninput={(e) => events.push((e.target as HTMLInputElement).name)}
	onsubmit={(e) => {
		e.preventDefault();
		posted = JSON.stringify([...new FormData(e.currentTarget as HTMLFormElement).entries()]);
	}}
>
	<Checkbox label="A" {name} value="a" checked />
	<Checkbox label="B" {name} value="b" checked />
	<button type="submit">Send</button>
</form>
<output data-testid="posted">{posted}</output>

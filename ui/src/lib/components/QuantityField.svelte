<script lang="ts">
	import Field from './form/Field.svelte';
	import { describedBy } from './form/util.ts';

	/** One free-text quantity (`16Gi`, `8`) with inline issues, shown as the user types. */
	let {
		label,
		value = $bindable(''),
		help,
		issues = [],
		placeholder
	}: {
		label: string;
		value?: string;
		help?: string;
		issues?: string[];
		placeholder?: string;
	} = $props();

	const uid = $props.id();
</script>

<Field id={uid} {label} {help} {issues}>
	<input
		class="ez-control ez-quantity"
		id={uid}
		type="text"
		autocomplete="off"
		spellcheck="false"
		{placeholder}
		bind:value
		aria-invalid={issues.length > 0 ? 'true' : undefined}
		aria-describedby={describedBy(uid, help, issues)}
	/>
</Field>

<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';
	import Field from './Field.svelte';
	import { describedBy, type Issues, messages } from './util.ts';

	/** Accepts the attributes `fields.x.as('text')` returns, so it can be spread onto it. */
	let {
		label,
		help,
		issues,
		id: idProp,
		value = $bindable(''),
		type = 'text',
		...rest
	}: {
		label: string;
		help?: string;
		issues?: Issues;
		value?: string;
	} & Omit<HTMLInputAttributes, 'value' | 'type'> & { type?: string } = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);
</script>

<Field {id} {label} {help} {issues}>
	<input
		class="ez-control"
		{...rest}
		{id}
		{type}
		bind:value
		aria-invalid={messages(issues).length > 0 || rest['aria-invalid'] === 'true'
			? 'true'
			: undefined}
		aria-describedby={describedBy(id, help, issues)}
	/>
</Field>

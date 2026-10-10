<script lang="ts">
	import type { HTMLTextareaAttributes } from 'svelte/elements';
	import Field from './Field.svelte';
	import { describedBy, type Issues, messages } from './util.ts';

	let {
		label,
		help,
		issues,
		id: idProp,
		value = $bindable(''),
		...rest
	}: {
		label: string;
		help?: string;
		issues?: Issues;
		value?: string;
	} & Omit<HTMLTextareaAttributes, 'value'> = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);
</script>

<Field {id} {label} {help} {issues}>
	<textarea
		class="ez-control"
		{...rest}
		{id}
		bind:value
		aria-invalid={messages(issues).length > 0 || rest['aria-invalid'] === 'true'
			? 'true'
			: undefined}
		aria-describedby={describedBy(id, help, issues)}></textarea>
</Field>

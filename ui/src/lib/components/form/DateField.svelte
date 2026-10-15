<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';
	import Field from './Field.svelte';
	import { describedBy, type Issues, messages } from './util.ts';

	/** A native date input styled like the other controls. `value` is `YYYY-MM-DD` (or empty). */
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
	} & Omit<HTMLInputAttributes, 'value' | 'type'> = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);
</script>

<Field {id} {label} {help} {issues}>
	<input
		class="ez-control"
		{...rest}
		{id}
		type="date"
		bind:value
		aria-invalid={messages(issues).length > 0 || rest['aria-invalid'] === 'true'
			? 'true'
			: undefined}
		aria-describedby={describedBy(id, help, issues)}
	/>
</Field>

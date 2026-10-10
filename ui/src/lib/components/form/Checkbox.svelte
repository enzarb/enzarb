<script lang="ts">
	import { Checkbox } from 'bits-ui';
	import { untrack } from 'svelte';
	import { formSync } from './sync.svelte.ts';
	import Icon from '../ui/Icon.svelte';
	import FieldMessages from './FieldMessages.svelte';
	import { describedBy, type Issues, messages } from './util.ts';

	/** Posts `value` under `name` when checked (like a native checkbox). */
	let {
		label,
		name,
		checked = $bindable(false),
		value = 'on',
		disabled = false,
		help,
		issues,
		id: idProp
	}: {
		label: string;
		name?: string;
		checked?: boolean;
		value?: string;
		disabled?: boolean;
		help?: string;
		issues?: Issues;
		id?: string;
	} = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);
	const initial = untrack(() => checked);
	const sync = formSync({
		name: () => name,
		value: () => checked,
		reset: () => (checked = initial),
		checkbox: true
	});
</script>

<div class="ez-field ez-choice" {@attach sync}>
	<Checkbox.Root
		{id}
		bind:checked
		{disabled}
		class="ez-checkbox"
		aria-labelledby="{id}-label"
		aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
		aria-describedby={describedBy(id, help, issues)}
	>
		{#snippet children({ checked })}{#if checked}<Icon name="check" />{/if}{/snippet}
	</Checkbox.Root>
	<label class="ez-choice__label" id="{id}-label" for={id}>{label}</label>
	{#if name && checked}<input type="hidden" {name} {value} />{/if}
	<div class="ez-choice__messages"><FieldMessages {id} {help} {issues} /></div>
</div>

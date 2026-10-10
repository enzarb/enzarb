<script lang="ts">
	import { Switch } from 'bits-ui';
	import { untrack } from 'svelte';
	import { formSync } from './sync.svelte.ts';
	import FieldMessages from './FieldMessages.svelte';
	import { describedBy, type Issues, messages } from './util.ts';

	/** On/off toggle; posts `value` under `name` while on. */
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

<div class="field choice" {@attach sync}>
	<Switch.Root
		{id}
		bind:checked
		{disabled}
		class="switch"
		aria-labelledby="{id}-label"
		aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
		aria-describedby={describedBy(id, help, issues)}
	>
		<Switch.Thumb class="thumb" />
	</Switch.Root>
	<label class="choice-label" id="{id}-label" for={id}>{label}</label>
	{#if name && checked}<input type="hidden" {name} {value} />{/if}
	<div class="choice-messages"><FieldMessages {id} {help} {issues} /></div>
</div>

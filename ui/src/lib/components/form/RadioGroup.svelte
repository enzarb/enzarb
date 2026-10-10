<script lang="ts">
	import { RadioGroup } from 'bits-ui';
	import { untrack } from 'svelte';
	import { formSync } from './sync.svelte.ts';
	import FieldMessages from './FieldMessages.svelte';
	import { describedBy, type Issues, messages, type Option } from './util.ts';

	let {
		label,
		options,
		value = $bindable(''),
		name,
		disabled = false,
		orientation = 'vertical',
		help,
		issues,
		id: idProp
	}: {
		label: string;
		options: Option[];
		value?: string;
		name?: string;
		disabled?: boolean;
		orientation?: 'vertical' | 'horizontal';
		help?: string;
		issues?: Issues;
		id?: string;
	} = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);
	const initial = untrack(() => value);
	const sync = formSync({ name: () => name, value: () => value, reset: () => (value = initial) });
</script>

<div class="field" {@attach sync}>
	<span class="field-label" id="{id}-label">{label}</span>
	<RadioGroup.Root
		{id}
		bind:value
		{disabled}
		{orientation}
		class="radio-group"
		aria-labelledby="{id}-label"
		aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
		aria-describedby={describedBy(id, help, issues)}
	>
		{#each options as o, n (o.value)}
			<div class="choice">
				<RadioGroup.Item
					id="{id}-{n}"
					value={o.value}
					disabled={o.disabled}
					class="radio"
					aria-labelledby="{id}-{n}-label"
				>
					{#snippet children({ checked })}<span aria-hidden="true">{checked ? '●' : ''}</span
						>{/snippet}
				</RadioGroup.Item>
				<label class="choice-label" id="{id}-{n}-label" for="{id}-{n}">
					{o.label}
					{#if o.description}<small class="muted">{o.description}</small>{/if}
				</label>
			</div>
		{/each}
	</RadioGroup.Root>
	{#if name && value}<input type="hidden" {name} {value} />{/if}
	<FieldMessages {id} {help} {issues} />
</div>

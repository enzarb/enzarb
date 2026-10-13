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

<div class="ez-field" {@attach sync}>
	<span class="ez-field__label" id="{id}-label">{label}</span>
	<RadioGroup.Root
		{id}
		bind:value
		{disabled}
		{orientation}
		class="ez-choice-group"
		aria-labelledby="{id}-label"
		aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
		aria-describedby={describedBy(id, help, issues)}
	>
		{#each options as o, n (o.value)}
			<div class="ez-choice">
				<RadioGroup.Item
					id="{id}-{n}"
					value={o.value}
					disabled={o.disabled}
					class="ez-radio"
					aria-labelledby="{id}-{n}-label"
				>
					{#snippet children({ checked })}{#if checked}<span class="ez-radio__dot"
							></span>{/if}{/snippet}
				</RadioGroup.Item>
				<label class="ez-choice__label" id="{id}-{n}-label" for="{id}-{n}">
					{o.label}
					{#if o.description}<span class="ez-choice__description">{o.description}</span>{/if}
				</label>
			</div>
		{/each}
	</RadioGroup.Root>
	{#if name && value}<input type="hidden" {name} {value} />{/if}
	<FieldMessages {id} {help} {issues} />
</div>

<script lang="ts">
	import { Select } from 'bits-ui';
	import { untrack } from 'svelte';
	import { formSync } from './sync.svelte.ts';
	import Field from './Field.svelte';
	import Icon from '../ui/Icon.svelte';
	import { describedBy, type Issues, messages, type Option } from './util.ts';

	/** Single-value listbox. The value is posted as a hidden input named `name`. */
	let {
		label,
		options,
		value = $bindable(''),
		name,
		placeholder = 'Select…',
		disabled = false,
		help,
		issues,
		id: idProp,
		onchange
	}: {
		label: string;
		options: Option[];
		value?: string;
		name?: string;
		placeholder?: string;
		disabled?: boolean;
		help?: string;
		issues?: Issues;
		id?: string;
		onchange?: (value: string) => void;
	} = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);
	const initial = untrack(() => value);
	const sync = formSync({ name: () => name, value: () => value, reset: () => (value = initial) });
	const selected = $derived(options.find((o) => o.value === value));
</script>

<div {@attach sync}>
	<Field {id} {label} {help} {issues}>
		<Select.Root
			type="single"
			bind:value
			{disabled}
			items={options}
			onValueChange={(v) => onchange?.(v)}
		>
			<Select.Trigger
				{id}
				class="ez-control ez-select__trigger"
				aria-labelledby="{id}-label {id}-value"
				aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
				aria-describedby={describedBy(id, help, issues)}
			>
				<span id="{id}-value" class:ez-subtle={!selected}>{selected?.label ?? placeholder}</span>
				<span class="ez-select__caret"><Icon name="chevron-down" /></span>
			</Select.Trigger>
			<Select.Portal>
				<Select.Content class="ez-popup" sideOffset={4}>
					<Select.Viewport>
						{#each options as o (o.value)}
							<Select.Item
								class="ez-option"
								value={o.value}
								label={o.label}
								disabled={o.disabled}
								aria-disabled={o.disabled ? 'true' : undefined}
							>
								{#snippet children({ selected })}
									<span class="ez-option__mark" aria-hidden="true"
										>{#if selected}<Icon name="check" />{/if}</span
									>
									<span>
										{o.label}
										{#if o.description}<span class="ez-option__description">{o.description}</span
											>{/if}
									</span>
								{/snippet}
							</Select.Item>
						{/each}
					</Select.Viewport>
				</Select.Content>
			</Select.Portal>
		</Select.Root>
		{#if name}<input type="hidden" {name} {value} />{/if}
	</Field>
</div>

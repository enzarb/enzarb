<script lang="ts">
	import { Select } from 'bits-ui';
	import { untrack } from 'svelte';
	import { formSync } from './sync.svelte.ts';
	import Field from './Field.svelte';
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

<div class="select" {@attach sync}>
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
				class="control select-trigger"
				aria-labelledby="{id}-label {id}-value"
				aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
				aria-describedby={describedBy(id, help, issues)}
			>
				<span id="{id}-value" class:muted={!selected}>{selected?.label ?? placeholder}</span>
				<span aria-hidden="true" class="caret">▾</span>
			</Select.Trigger>
			<Select.Portal>
				<Select.Content class="popup" sideOffset={4}>
					<Select.Viewport>
						{#each options as o (o.value)}
							<Select.Item
								class="option"
								value={o.value}
								label={o.label}
								disabled={o.disabled}
								aria-disabled={o.disabled ? 'true' : undefined}
							>
								{#snippet children({ selected })}
									<span class="option-mark" aria-hidden="true">{selected ? '✓' : ''}</span>
									<span class="option-text">
										{o.label}
										{#if o.description}<small class="muted">{o.description}</small>{/if}
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

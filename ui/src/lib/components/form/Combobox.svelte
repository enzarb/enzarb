<script lang="ts">
	import { Combobox } from 'bits-ui';
	import { tick, untrack } from 'svelte';
	import { SvelteMap } from 'svelte/reactivity';
	import { formSync } from './sync.svelte.ts';
	import Field from './Field.svelte';
	import Icon from '../ui/Icon.svelte';
	import { describedBy, type Issues, messages, type Option } from './util.ts';

	/**
	 * Searchable multi-select. Options may carry a `group` (rendered under a heading), be
	 * `disabled`, and have a `description` (e.g. why they are disabled). Every selected value
	 * is posted as its own hidden input named `name`.
	 */
	let {
		label,
		options,
		value = $bindable([]),
		name,
		placeholder = 'Search…',
		emptyText = 'No matches',
		disabled = false,
		help,
		issues,
		id: idProp
	}: {
		label: string;
		options: Option[];
		value?: string[];
		name?: string;
		placeholder?: string;
		emptyText?: string;
		disabled?: boolean;
		help?: string;
		issues?: Issues;
		id?: string;
	} = $props();

	const uid = $props.id();
	const id = $derived(idProp ?? uid);

	let query = $state('');
	let open = $state(false);
	let box = $state<HTMLElement>();
	let announcement = $state('');

	const initial = untrack(() => [...value]);
	const sync = formSync({
		name: () => name,
		value: () => value,
		reset: () => (value = [...initial])
	});

	const shown = $derived.by(() => {
		const q = query.trim().toLowerCase();
		return options.filter(
			(o) => !q || `${o.label} ${o.description ?? ''} ${o.group ?? ''}`.toLowerCase().includes(q)
		);
	});
	/** Shown options bucketed by group, in first-seen order; '' is the ungrouped bucket. */
	const groups = $derived.by(() => {
		const map = new SvelteMap<string, Option[]>();
		for (const o of shown) map.set(o.group ?? '', [...(map.get(o.group ?? '') ?? []), o]);
		return [...map];
	});
	const chosen = $derived(
		value.map((v) => options.find((o) => o.value === v) ?? { value: v, label: v })
	);

	/** Remove a chip. From a chip button, focus moves to the next chip, else the input. */
	async function remove(v: string, fromChip = false) {
		const index = value.indexOf(v);
		const removed = chosen[index]?.label ?? v;
		value = value.filter((x) => x !== v);
		announcement = `${removed} removed`;
		if (!fromChip) return;
		await tick();
		const next = box?.querySelectorAll<HTMLElement>('.ez-chip__remove')[index];
		(next ?? box?.querySelector<HTMLElement>('.ez-combobox__input'))?.focus();
	}
	/**
	 * bits-ui writes the picked label into the input right after reporting the new value, so
	 * clear the text once that has settled; the DOM value is reset too because bits keeps its
	 * own copy when `query` was already empty.
	 */
	function clearInput() {
		void tick().then(() => {
			if (!box?.isConnected) return;
			query = '';
			const input = box?.querySelector<HTMLInputElement>('.ez-combobox__input');
			if (input) input.value = '';
		});
	}
	function onkeydown(e: KeyboardEvent & { currentTarget: HTMLInputElement }) {
		if (e.key === 'Backspace' && e.currentTarget.value === '' && value.length)
			void remove(value[value.length - 1]);
	}
</script>

<div {@attach sync}>
	<Field {id} {label} {help} {issues}>
		<Combobox.Root
			type="multiple"
			bind:value
			bind:open
			inputValue={query}
			{disabled}
			onValueChange={clearInput}
			onOpenChange={(o) => {
				if (!o) query = '';
			}}
		>
			<div class="ez-control ez-combobox" class:ez-control--disabled={disabled} bind:this={box}>
				{#each chosen as c (c.value)}
					<span class="ez-chip">
						{c.label}
						<button
							type="button"
							class="ez-chip__remove"
							aria-label="Remove {c.label}"
							{disabled}
							onclick={() => remove(c.value, true)}><Icon name="x" /></button
						>
					</span>
				{/each}
				<Combobox.Input
					{id}
					{placeholder}
					class="ez-combobox__input"
					aria-labelledby="{id}-label"
					aria-invalid={messages(issues).length > 0 ? 'true' : undefined}
					aria-describedby={describedBy(id, help, issues)}
					oninput={(e) => (query = e.currentTarget.value)}
					{onkeydown}
				/>
				<Combobox.Trigger class="ez-combobox__trigger" aria-label="Show options">
					<Icon name="chevron-down" />
				</Combobox.Trigger>
			</div>
			<Combobox.Portal>
				<Combobox.Content class="ez-popup" sideOffset={4}>
					<Combobox.Viewport>
						{#each groups as [group, items] (group)}
							{#snippet rows()}
								{#each items as o (o.value)}
									<Combobox.Item
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
												{#if o.description}<span class="ez-option__description"
														>{o.description}</span
													>{/if}
											</span>
										{/snippet}
									</Combobox.Item>
								{/each}
							{/snippet}
							{#if group}
								<Combobox.Group>
									<Combobox.GroupHeading class="ez-popup__group-heading"
										>{group}</Combobox.GroupHeading
									>
									{@render rows()}
								</Combobox.Group>
							{:else}
								{@render rows()}
							{/if}
						{:else}
							<p class="ez-popup__empty">{emptyText}</p>
						{/each}
					</Combobox.Viewport>
				</Combobox.Content>
			</Combobox.Portal>
		</Combobox.Root>
		{#if name}{#each value as v (v)}<input type="hidden" {name} value={v} />{/each}{/if}
	</Field>
	<span class="ez-sr-only" aria-live="polite">{announcement}</span>
</div>

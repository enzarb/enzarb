<script lang="ts" module>
	import type { Snippet } from 'svelte';
	import type { IconName } from './Icon.svelte';

	export type MenuEntry =
		| {
				type: 'item';
				label: string;
				icon?: IconName;
				/** Renders a link instead of an action. */
				href?: string;
				onselect?: () => void;
				tone?: 'danger';
				disabled?: boolean;
		  }
		| { type: 'separator' }
		| { type: 'label'; text: string }
		/** A single-choice group (e.g. the theme). */
		| {
				type: 'radio';
				/** Names the group (rendered as its heading). */
				label: string;
				value: string;
				onchange: (value: string) => void;
				options: { value: string; label: string; icon?: IconName }[];
		  };
</script>

<script lang="ts">
	import { DropdownMenu } from 'bits-ui';
	import Icon from './Icon.svelte';

	/**
	 * Dropdown menu on bits-ui: Enter/Space/ArrowDown open it, arrow keys, Home/End and
	 * type-ahead move, Enter selects, Escape closes and returns focus to the trigger.
	 * Describe the content as `entries`; `header` is a non-interactive block above them.
	 */
	let {
		trigger,
		label,
		entries,
		header,
		align = 'end'
	}: {
		/** Content of the trigger button. */
		trigger: Snippet;
		/** Accessible name of the trigger when it has no text. */
		label?: string;
		entries: MenuEntry[];
		header?: Snippet;
		align?: 'start' | 'center' | 'end';
	} = $props();
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger class="ez-btn ez-btn--ghost" aria-label={label}>
		{@render trigger()}
	</DropdownMenu.Trigger>
	<DropdownMenu.Portal>
		<DropdownMenu.Content class="ez-menu" {align} sideOffset={6}>
			{#if header}<div class="ez-menu__header">{@render header()}</div>{/if}
			{#each entries as e, n (n)}
				{#if e.type === 'separator'}
					<DropdownMenu.Separator class="ez-menu__separator" />
				{:else if e.type === 'label'}
					<div class="ez-menu__label">{e.text}</div>
				{:else if e.type === 'radio'}
					<DropdownMenu.Group>
						<DropdownMenu.GroupHeading class="ez-menu__label">{e.label}</DropdownMenu.GroupHeading>
						<DropdownMenu.RadioGroup value={e.value} onValueChange={e.onchange}>
							{#each e.options as o (o.value)}
								<DropdownMenu.RadioItem
									class="ez-menu__item {o.value === e.value ? 'ez-menu__item--checked' : ''}"
									value={o.value}
									closeOnSelect={false}
								>
									{#if o.icon}<Icon name={o.icon} />{/if}
									<span>{o.label}</span>
									<span class="ez-menu__check"
										>{#if o.value === e.value}<Icon name="check" />{/if}</span
									>
								</DropdownMenu.RadioItem>
							{/each}
						</DropdownMenu.RadioGroup>
					</DropdownMenu.Group>
				{:else if e.href}
					<DropdownMenu.Item class="ez-menu__item" disabled={e.disabled}>
						{#snippet child({ props })}
							<a {...props} href={e.href}>
								{#if e.icon}<Icon name={e.icon} />{/if}
								{e.label}
							</a>
						{/snippet}
					</DropdownMenu.Item>
				{:else}
					<DropdownMenu.Item
						class="ez-menu__item {e.tone === 'danger' ? 'ez-menu__item--danger' : ''}"
						disabled={e.disabled}
						onSelect={() => e.onselect?.()}
					>
						{#if e.icon}<Icon name={e.icon} />{/if}
						{e.label}
					</DropdownMenu.Item>
				{/if}
			{/each}
		</DropdownMenu.Content>
	</DropdownMenu.Portal>
</DropdownMenu.Root>

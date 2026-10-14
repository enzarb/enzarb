<script lang="ts">
	import { Dialog } from 'bits-ui';
	import type { Snippet } from 'svelte';

	/**
	 * Modal dialog on bits-ui: focus is trapped inside while open, Escape and the overlay close it,
	 * and focus returns to the opener. `title` is required (it names the dialog); `footer` is the
	 * button row. Bind `open`, or pass a `trigger` snippet that receives the trigger props.
	 */
	let {
		open = $bindable(false),
		title,
		description,
		wide = false,
		trigger,
		footer,
		children
	}: {
		open?: boolean;
		title: string;
		description?: string;
		/** A wider panel for content that is read, such as a plan or a review. */
		wide?: boolean;
		trigger?: Snippet<[Record<string, unknown>]>;
		footer?: Snippet;
		children?: Snippet;
	} = $props();
</script>

<Dialog.Root bind:open>
	{#if trigger}
		<Dialog.Trigger>
			{#snippet child({ props })}{@render trigger(props)}{/snippet}
		</Dialog.Trigger>
	{/if}
	<Dialog.Portal>
		<Dialog.Overlay class="ez-dialog__overlay" />
		<Dialog.Content
			class={wide ? 'ez-dialog__content ez-dialog__content--wide' : 'ez-dialog__content'}
		>
			<div>
				<Dialog.Title class="ez-dialog__title">{title}</Dialog.Title>
				{#if description}
					<Dialog.Description class="ez-dialog__description">{description}</Dialog.Description>
				{/if}
			</div>
			{#if children}<div class="ez-dialog__body">{@render children()}</div>{/if}
			{#if footer}<div class="ez-dialog__footer">{@render footer()}</div>{/if}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>

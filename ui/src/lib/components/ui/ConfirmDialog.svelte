<script lang="ts">
	import { Dialog } from 'bits-ui';
	import Button from '../form/Button.svelte';
	import DialogBase from './Dialog.svelte';

	/**
	 * "Are you sure?" for destructive or irreversible actions. `onconfirm` may be async: the
	 * confirm button shows its loading state until it settles, then the dialog closes (errors are
	 * the caller's to show). Cancel, Escape and the overlay close it without calling `onconfirm`.
	 */
	let {
		open = $bindable(false),
		title,
		description,
		confirmLabel = 'Confirm',
		cancelLabel = 'Cancel',
		tone = 'danger',
		onconfirm
	}: {
		open?: boolean;
		title: string;
		description?: string;
		confirmLabel?: string;
		cancelLabel?: string;
		tone?: 'danger' | 'primary';
		onconfirm: () => void | Promise<void>;
	} = $props();

	let pending = $state(false);

	async function confirm() {
		pending = true;
		try {
			await onconfirm();
		} finally {
			pending = false;
			open = false;
		}
	}
</script>

<DialogBase bind:open {title} {description}>
	{#snippet footer()}
		<Dialog.Close disabled={pending}>
			{#snippet child({ props })}
				<Button {...props} variant="secondary">{cancelLabel}</Button>
			{/snippet}
		</Dialog.Close>
		<Button variant={tone} loading={pending} onclick={confirm}>{confirmLabel}</Button>
	{/snippet}
</DialogBase>

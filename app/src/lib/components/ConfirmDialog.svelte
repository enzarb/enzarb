<script lang="ts">
	import { confirmStore, settle } from '$lib/confirm';

	let dialog: HTMLDialogElement | undefined = $state();
	let typed = $state('');

	// Open/close the native dialog in response to the store, so any caller of
	// confirm() drives this single instance.
	$effect(() => {
		if (!dialog) return;
		if ($confirmStore.open && !dialog.open) {
			typed = '';
			dialog.showModal();
		} else if (!$confirmStore.open && dialog.open) {
			dialog.close();
		}
	});

	const requireText = $derived($confirmStore.requireText);
	const confirmDisabled = $derived(!!requireText && typed !== requireText);
</script>

<dialog
	bind:this={dialog}
	class="confirm-dialog"
	oncancel={(e) => {
		e.preventDefault();
		settle(false);
	}}
	onclose={() => settle(false)}
>
	<h3>{$confirmStore.title}</h3>
	{#if $confirmStore.message}
		<p>{$confirmStore.message}</p>
	{/if}
	{#if requireText}
		<!-- svelte-ignore a11y_autofocus -->
		<label class="require">
			Type <code>{requireText}</code> to confirm
			<input
				type="text"
				autocomplete="off"
				autocapitalize="off"
				spellcheck="false"
				autofocus
				bind:value={typed}
				onkeydown={(e) => {
					if (e.key === 'Enter' && !confirmDisabled) settle(true);
				}}
			/>
		</label>
	{/if}
	<div class="actions">
		<button type="button" class="btn" onclick={() => settle(false)}>
			{$confirmStore.cancelText ?? 'Cancel'}
		</button>
		<button
			type="button"
			class="btn {$confirmStore.danger ? 'btn-danger' : 'btn-primary'}"
			disabled={confirmDisabled}
			onclick={() => settle(true)}
		>
			{$confirmStore.confirmText ?? 'Confirm'}
		</button>
	</div>
</dialog>

<style>
	.confirm-dialog {
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
		background: var(--color-surface);
		color: var(--color-text);
		padding: 1.5rem;
		max-width: 440px;
		width: calc(100vw - 2rem);
		box-shadow: var(--shadow);
	}
	.confirm-dialog::backdrop {
		background: rgba(0, 0, 0, 0.5);
	}
	.confirm-dialog h3 {
		margin: 0 0 0.5rem;
		font-size: 16px;
	}
	.confirm-dialog p {
		margin: 0 0 1.25rem;
		color: var(--color-text-muted);
		font-size: 14px;
		line-height: 1.5;
		white-space: pre-line;
	}
	.require {
		display: block;
		font-size: 13px;
		color: var(--color-text-muted);
		margin-bottom: 1.25rem;
	}
	.require code {
		font-family: var(--font-mono);
		color: var(--color-text);
		background: var(--color-surface-2);
		padding: 0.05rem 0.3rem;
		border-radius: 4px;
	}
	.require input {
		display: block;
		width: 100%;
		margin-top: 0.5rem;
		padding: 0.4rem 0.5rem;
		font-size: 14px;
		background: var(--color-surface-2);
		color: var(--color-text);
		border: 1px solid var(--color-border);
		border-radius: var(--radius);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: 0.5rem;
	}
	.actions .btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
</style>

<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import TextArea from '#lib/components/form/TextArea.svelte';
	import ConfirmDialog from '#lib/components/ui/ConfirmDialog.svelte';
	import { commentKind } from '#lib/tasks.ts';

	/**
	 * Stop, resume, cancel, and a comment box that steers the running agent (or comments while the
	 * task is not running). The engine decides whether each one applies; buttons are shown by engine
	 * phase only.
	 */
	let {
		status,
		onsubmit
	}: {
		status: string;
		onsubmit: (
			kind: 'stop' | 'resume' | 'cancel' | 'steer' | 'comment',
			body?: string
		) => Promise<unknown>;
	} = $props();

	let body = $state('');
	let pending = $state<string | null>(null);
	let message = $state('');
	let confirmCancel = $state(false);
	const kind = $derived(commentKind(status));

	async function send(k: 'stop' | 'resume' | 'cancel' | 'steer' | 'comment', text?: string) {
		pending = k;
		message = '';
		try {
			await onsubmit(k, text);
			if (text !== undefined) body = '';
			message = `Sent ${k}.`;
		} catch {
			message = `Could not send ${k}.`;
		} finally {
			pending = null;
		}
	}
</script>

<div class="ez-stack">
	{#if status !== 'ended'}
		<div class="ez-cluster" role="group" aria-label="Task controls">
			{#if status === 'running'}
				<Button size="sm" loading={pending === 'stop'} onclick={() => send('stop')}
					>Stop step</Button
				>
			{/if}
			{#if status === 'paused'}
				<Button
					size="sm"
					variant="primary"
					loading={pending === 'resume'}
					onclick={() => send('resume')}>Resume</Button
				>
			{/if}
			<Button size="sm" variant="danger" onclick={() => (confirmCancel = true)}>Cancel task</Button>
		</div>
	{/if}
	<form
		class="ez-stack"
		onsubmit={(e) => {
			e.preventDefault();
			if (body.trim()) void send(kind, body);
		}}
	>
		<TextArea
			label={kind === 'steer' ? 'Steer the running agent' : 'Comment'}
			help={kind === 'steer'
				? 'Delivered into the running session as a user turn.'
				: 'Posted as a comment; /commands work here.'}
			bind:value={body}
			rows={3}
		/>
		<div class="ez-cluster">
			<Button
				type="submit"
				variant="primary"
				size="sm"
				loading={pending === kind}
				disabled={!body.trim()}>{kind === 'steer' ? 'Send steer' : 'Comment'}</Button
			>
			{#if message}<span class="ez-muted" role="status">{message}</span>{/if}
		</div>
	</form>
</div>
<ConfirmDialog
	bind:open={confirmCancel}
	tone="danger"
	title="Cancel this task?"
	description="The engine ends the task; running steps are stopped."
	confirmLabel="Cancel task"
	onconfirm={() => send('cancel')}
/>

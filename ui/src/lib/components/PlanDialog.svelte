<script lang="ts">
	import { untrack } from 'svelte';
	import { COMMENT_MAX_LENGTH, type WorkAction, type WorkItemSummary } from '#lib/work-items.ts';
	import WorkActions from './WorkActions.svelte';
	import Button from './form/Button.svelte';
	import TextArea from './form/TextArea.svelte';
	import Dialog from './ui/Dialog.svelte';
	import Notice from './ui/Notice.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/**
	 * An issue's plan in a dialog: read it, approve or cancel, or send feedback for a revised plan
	 * without leaving enzarb. `load` returns the sanitized plan HTML (null when there is none yet);
	 * it runs each time the dialog opens. A successful action closes the dialog; the board moves
	 * the card when the live update arrives. Errors stay in the dialog.
	 */
	let {
		open = $bindable(false),
		item,
		load,
		onaction,
		oncomment
	}: {
		open?: boolean;
		item: WorkItemSummary | null;
		load: (id: number) => Promise<string | null>;
		onaction: (id: number, action: WorkAction) => Promise<void>;
		oncomment: (id: number, body: string) => Promise<void>;
	} = $props();

	const id = $derived(item?.id ?? null);

	let html = $state<string | null>(null);
	let status = $state<'loading' | 'done' | 'error'>('loading');
	let feedback = $state('');
	let pending = $state<WorkAction | null>(null);
	let error = $state('');

	$effect(() => {
		if (!open || id === null) return;
		const wanted = id;
		untrack(() => {
			status = 'loading';
			html = null;
			feedback = '';
			error = '';
		});
		let current = true;
		load(wanted).then(
			(h) => {
				if (!current) return;
				html = h;
				status = 'done';
			},
			() => {
				if (current) status = 'error';
			}
		);
		return () => (current = false);
	});

	const body = $derived(feedback.trim());

	async function run(action: WorkAction, send: (id: number) => Promise<void>) {
		if (id === null) return;
		pending = action;
		error = '';
		try {
			await send(id);
			open = false;
		} catch {
			error =
				action === 'comment'
					? 'Could not send your feedback. The plan may have changed.'
					: `Could not ${action} this item. Its state may have changed.`;
		} finally {
			pending = null;
		}
	}
</script>

<Dialog bind:open wide title={item ? `Plan: ${item.title}` : 'Plan'} description={item?.key}>
	{#if status === 'loading'}
		<Skeleton label="Loading plan" rows={5} />
	{:else if status === 'error'}
		<Notice tone="error">Could not load the plan.</Notice>
	{:else if html}
		<!-- Sanitized on the server (sanitize-html allow-list) before it is sent. -->
		<!-- eslint-disable-next-line svelte/no-at-html-tags -->
		<div class="ez-markdown ez-dialog__scroll">{@html html}</div>
	{:else}
		<p class="ez-muted">No plan yet.</p>
	{/if}
	<div class="ez-dialog__section">
		<TextArea
			label="Feedback on the plan"
			help="Posted to the issue; enzarb revises the plan."
			rows={4}
			maxlength={COMMENT_MAX_LENGTH}
			bind:value={feedback}
		/>
	</div>
	{#if error}<Notice tone="error">{error}</Notice>{/if}
	{#snippet footer()}
		<Button variant="ghost" onclick={() => (open = false)}>Close</Button>
		<Button
			variant="secondary"
			disabled={!body || pending !== null}
			loading={pending === 'comment'}
			onclick={() => run('comment', (i) => oncomment(i, body))}>Request revision</Button
		>
		<WorkActions
			actions={['approve', 'cancel']}
			{pending}
			onaction={(a) => run(a, (i) => onaction(i, a))}
		/>
	{/snippet}
</Dialog>

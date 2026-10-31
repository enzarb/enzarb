<script lang="ts">
	import type { ActionOutcome } from '#lib/tasks.ts';
	import Button from '../form/Button.svelte';
	import TextArea from '../form/TextArea.svelte';
	import Section from '../ui/Section.svelte';

	/**
	 * The plan a run waits on, rendered, with Approve (the default) and Revise. `html` is the
	 * server's sanitised rendering of the markdown plan. Failures show here; the host shows the
	 * confirmation. Shared by the Tasks detail and the run page.
	 */
	let {
		html,
		answer,
		stop
	}: {
		html: string;
		answer: (action: 'approve' | 'revise', text?: string) => Promise<ActionOutcome>;
		/** Stop for now, kept as a secondary action. */
		stop?: () => void;
	} = $props();

	let revising = $state(false);
	let text = $state('');
	let busy = $state<'approve' | 'revise' | null>(null);
	let problem = $state('');

	async function send(action: 'approve' | 'revise') {
		busy = action;
		problem = '';
		try {
			const r = await answer(action, action === 'revise' ? text : undefined);
			if (!r.ok) problem = r.message;
			else {
				text = '';
				revising = false;
			}
		} catch {
			problem = 'That did not work. Try again.';
		} finally {
			busy = null;
		}
	}
</script>

<Section
	title="Plan awaiting approval"
	description="Approve it to start the implementation, or ask for changes."
>
	<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized by renderMarkdown on the server -->
	<div class="ez-md" data-testid="plan-body">{@html html}</div>
	{#if problem}<p class="ez-noderuns__error" role="alert">{problem}</p>{/if}
	<div class="ez-tasks__actions">
		<Button
			variant="primary"
			loading={busy === 'approve'}
			disabled={busy !== null}
			onclick={() => send('approve')}>Approve</Button
		>
		<Button disabled={busy !== null} onclick={() => (revising = !revising)}>Revise</Button>
		{#if stop}<Button disabled={busy !== null} onclick={stop}>Stop for now</Button>{/if}
	</div>
	{#if revising}
		<form
			class="ez-tasks__revise"
			onsubmit={(e) => {
				e.preventDefault();
				void send('revise');
			}}
		>
			<TextArea
				label="What should change in the plan?"
				rows={3}
				bind:value={text}
				maxlength={4000}
			/>
			<Button type="submit" loading={busy === 'revise'} disabled={busy !== null || !text.trim()}
				>Send revision</Button
			>
		</form>
	{/if}
</Section>

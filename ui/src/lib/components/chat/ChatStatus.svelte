<script lang="ts">
	import { buildChatSteps, type ChatProgress, type ChatStepState } from '#lib/chat-steps.ts';
	import type { ConnectionAction } from '#lib/acp/connection.ts';
	import StepList, { type StepItem, type StepStatus } from '../ui/StepList.svelte';

	/** Connection checklist plus the one action the current state offers. */
	let {
		progress,
		message = '',
		action = null,
		onAction
	}: {
		progress: ChatProgress;
		message?: string;
		action?: ConnectionAction;
		onAction: (action: Exclude<ConnectionAction, null | 'set_up'>) => void;
	} = $props();

	const status: Record<ChatStepState, StepStatus> = {
		pending: 'pending',
		active: 'active',
		done: 'done',
		error: 'failed'
	};
	const words: Record<ChatStepState, string> = {
		pending: 'Pending',
		active: 'In progress',
		done: 'Done',
		error: 'Failed'
	};
	const labels: Record<Exclude<ConnectionAction, null | 'set_up'>, string> = {
		retry: 'Retry',
		reconnect: 'Reconnect',
		take_over: 'Take over'
	};

	const items = $derived<StepItem[]>(
		buildChatSteps(progress).map((step) => ({
			id: step.id,
			status: status[step.state],
			label: step.label,
			stateText: words[step.state],
			message: step.message || undefined,
			attrs: { 'data-state': step.state }
		}))
	);
</script>

<div class="ez-chat-status">
	<StepList {items} label="Chat connection progress" />
	{#if message}<p class="ez-chat-status__message" role="status">{message}</p>{/if}
	{#if action === 'set_up'}
		<a class="ez-btn ez-btn--primary ez-btn--sm" href="/settings#claude">Set up your Claude token</a
		>
	{:else if action}
		<button
			type="button"
			class="ez-btn ez-btn--primary ez-btn--sm"
			onclick={() => onAction(action)}
		>
			{labels[action]}
		</button>
	{/if}
</div>

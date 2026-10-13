<script lang="ts">
	import type {
		PermissionChoice,
		PermissionStatus,
		PermissionOption,
		ToolView
	} from '#lib/acp/types.ts';

	/**
	 * An agent's request to run a tool. Offers only "Allow once" and "Deny", and only while pending;
	 * nothing is preselected or focused. `epoch` is the connection epoch the item was rendered in.
	 */
	let {
		id,
		toolCall,
		options,
		status,
		hint,
		reason,
		epoch,
		onAnswer
	}: {
		id: string;
		toolCall: ToolView;
		options: PermissionOption[];
		status: PermissionStatus;
		hint?: string;
		reason?: string;
		epoch: number;
		onAnswer: (id: string, choice: PermissionChoice, epoch: number) => void;
	} = $props();

	const canAllow = $derived(options.some((o) => o.kind === 'allow_once'));
	const canDeny = $derived(options.some((o) => o.kind === 'reject_once'));
	const words: Record<Exclude<PermissionStatus, 'pending' | 'blocked'>, string> = {
		allowed: 'Allowed',
		denied: 'Denied',
		cancelled: 'Cancelled'
	};
</script>

<div class="ez-chat-perm" data-status={status}>
	<p class="ez-chat-perm__title">
		{status === 'blocked' ? 'Blocked action' : 'Permission requested'}: {toolCall.title ||
			'Tool call'}
	</p>
	{#if toolCall.text}<pre class="ez-chat-tool__out">{toolCall.text}</pre>{/if}
	{#if status === 'blocked'}
		<p class="ez-chat-perm__state">Blocked by sandbox policy: {reason}</p>
	{:else if hint}
		<p class="ez-chat-perm__hint">{hint}</p>
	{/if}
	{#if status === 'pending'}
		{#if canAllow || canDeny}
			<div class="ez-chat-perm__actions" role="group" aria-label="Respond to permission request">
				{#if canAllow}
					<button
						type="button"
						class="ez-btn ez-btn--secondary ez-btn--sm"
						onclick={() => onAnswer(id, 'allow', epoch)}
					>
						Allow once
					</button>
				{/if}
				{#if canDeny}
					<button
						type="button"
						class="ez-btn ez-btn--secondary ez-btn--sm"
						onclick={() => onAnswer(id, 'deny', epoch)}
					>
						Deny
					</button>
				{/if}
			</div>
		{/if}
	{:else if status !== 'blocked'}
		<p class="ez-chat-perm__state">{words[status]}</p>
	{/if}
</div>

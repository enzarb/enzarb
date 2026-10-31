<script lang="ts">
	import type { ConnectionDeps } from '#lib/acp/connection.ts';
	import type { ChatItem } from '#lib/acp/types.ts';
	import type { RunDetail } from '#lib/runs.ts';
	import { splitContext } from '#lib/rules-chat.ts';
	import type { Task } from '#lib/tasks.ts';
	import { cloneUrl, taskCheckout, taskContextMessage, taskStoreKey } from '#lib/task-chat.ts';
	import Chat from '../chat/Chat.svelte';

	/**
	 * The chat pinned under a task's detail: an ez agent working on the task's own branch (a run's
	 * branch, a pull request's head, a new branch for an issue). The composer is always in view; the
	 * transcript opens upward over the detail when there is something to read. One chat per task:
	 * switching tasks switches chats. The agent cannot push (commits stay in the sandbox until a run
	 * pushes them).
	 */
	let {
		task,
		run,
		connect
	}: {
		task: Task;
		run?: RunDetail | null;
		/** Override the sandbox connection (tests). */
		connect?: ConnectionDeps['connect'];
	} = $props();

	const checkout = $derived(taskCheckout(task, run));
	// `Chat` reads its checkout once per mount, so a run task waits for its run: the branch a node
	// reported is not known before. The branch is also part of the key (a different one remounts).
	const needsRun = $derived(task.kind === 'wait' || task.kind === 'failed');

	let open = $state(false);
	let count = 0;
	function onItems(items: ChatItem[]) {
		// New content (a reply, a permission request) always opens the transcript.
		if (items.length > count) open = true;
		count = items.length;
	}
</script>

{#if needsRun && run === undefined}
	<p class="ez-muted" data-testid="task-chat-bar" role="status">Loading the run…</p>
{:else if needsRun && run === null}
	<p class="ez-muted" data-testid="task-chat-bar">
		The run could not be read, so its branch is not known. Chat is unavailable for now.
	</p>
{:else}
	{#key `${task.key}:${checkout.branch}`}
		<div class="ez-taskchat" data-testid="task-chat-bar" data-open={open}>
			<div class="ez-taskchat__bar">
				<button
					type="button"
					class="ez-btn ez-btn--secondary ez-btn--sm"
					aria-expanded={open}
					onclick={() => (open = !open)}>{open ? 'Hide conversation' : 'Show conversation'}</button
				>
				<span class="ez-muted ez-taskchat__note"
					>Changes stay in the sandbox until a run pushes them.</span
				>
			</div>
			<Chat
				project={task.project}
				repo={cloneUrl(task.repo)}
				{checkout}
				storeKey={taskStoreKey(task)}
				{connect}
				wrapFirst={(text) => `${taskContextMessage(task, run)}\n\n${text}`}
				hideContext={splitContext}
				contextLabel="Context: the task and what its run did (show)"
				{onItems}
			/>
		</div>
	{/key}
{/if}

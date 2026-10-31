<script lang="ts">
	import { repoContext } from '#lib/github-links.ts';
	import type { RunDetail } from '#lib/runs.ts';
	import { groupTasks, type DefaultActionId, type Task, type TaskList } from '#lib/tasks.ts';
	import Button from './form/Button.svelte';
	import TaskDetail from './tasks/TaskDetail.svelte';
	import TaskSidebar from './tasks/TaskSidebar.svelte';
	import EmptyState from './ui/EmptyState.svelte';
	import Notice from './ui/Notice.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/**
	 * Everything that waits on a human: the list on the left, the selected task on the right
	 * (`?task=<key>`, set by the page). On a phone the two stack, list first.
	 */
	let {
		list,
		selected = null,
		href,
		backHref,
		run,
		showProject = false,
		failed = false,
		retry,
		implement,
		retryRun,
		stop
	}: {
		/** `undefined` while the first load is in flight. */
		list: TaskList | undefined;
		/** The `?task=` key from the URL. */
		selected?: string | null;
		/** The URL that selects a task key. */
		href: (key: string) => string;
		/** The URL with no task selected (the back control on a phone). */
		backHref: string;
		/** The selected run task's run; see TaskDetail. */
		run?: RunDetail | null;
		/** The top-level page spans Projects: show which one a task belongs to. */
		showProject?: boolean;
		failed?: boolean;
		retry: () => void;
		implement: (t: Task) => Promise<string>;
		retryRun: (t: Task) => Promise<string>;
		stop: (t: Task) => Promise<string>;
	} = $props();

	let busy = $state<string | null>(null);
	let message = $state('');

	async function act(t: Task, id: DefaultActionId) {
		const run =
			id === 'implement' ? implement : id === 'retry' ? retryRun : id === 'stop' ? stop : null;
		if (!run) return;
		busy = t.key;
		message = '';
		try {
			message = await run(t);
		} catch {
			message = 'That did not work. Try again.';
		} finally {
			busy = null;
		}
	}

	const groups = $derived(list ? groupTasks(list.tasks) : []);
	const current = $derived(list?.tasks.find((t) => t.key === selected) ?? null);
	const context = $derived(repoContext((list?.tasks ?? []).map((t) => t.repo)));
</script>

{#if failed}
	<Notice tone="error" title="Could not load tasks">
		Something went wrong while loading the tasks.
		{#snippet actions()}<Button onclick={retry}>Retry</Button>{/snippet}
	</Notice>
{:else if list === undefined}
	<Skeleton label="Loading tasks" rows={3} />
{:else}
	{#if message}<div role="status" class="ez-muted">{message}</div>{/if}
	{#each list.notes as note (note)}
		<Notice tone="warning" title="Some tasks may be missing">{note}</Notice>
	{/each}
	{#if groups.length === 0}
		<EmptyState
			title="Nothing is waiting for you"
			description="Runs parked at a wait, failed runs, pull requests from ez branches and triaged issues show up here."
		/>
	{:else}
		<div class="ez-tasks" data-selected={current ? 'true' : 'false'}>
			<TaskSidebar {groups} selected={current?.key ?? null} {href} {showProject} {context} />
			<div class="ez-tasks__pane">
				{#if current}
					<a class="ez-tasks__back" href={backHref}>All tasks</a>
					<TaskDetail
						task={current}
						{run}
						{context}
						busy={busy === current.key}
						act={(id) => act(current, id)}
					/>
				{:else}
					<p class="ez-muted ez-tasks__none">Select a task to see it here.</p>
				{/if}
			</div>
		</div>
	{/if}
{/if}

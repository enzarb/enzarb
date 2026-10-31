<script lang="ts">
	import { untrack } from 'svelte';
	import { repoContext } from '#lib/github-links.ts';
	import type { MergeOutcome, MergeStep, PullReview } from '#lib/pull-review.ts';
	import type { RunDetail } from '#lib/runs.ts';
	import {
		groupTasks,
		IMPLEMENT_WORKFLOW,
		type ActionOutcome,
		type DefaultActionId,
		type Task,
		type TaskList
	} from '#lib/tasks.ts';
	import Button from './form/Button.svelte';
	import RunLink from './RunLink.svelte';
	import MergeSteps from './tasks/MergeSteps.svelte';
	import PlanApproval from './tasks/PlanApproval.svelte';
	import PullReviewView from './tasks/PullReviewView.svelte';
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
		plan = null,
		review,
		showProject = false,
		failed = false,
		retry,
		implement,
		retryRun,
		stop,
		answerPlan,
		mergePull
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
		/** The rendered plan of the selected waiting run, when it waits at a plan approval. */
		plan?: { html: string } | null;
		/** The selected pull request's diff and notes: `undefined` loading, `null` unreadable. */
		review?: PullReview | null;
		/** The top-level page spans Projects: show which one a task belongs to. */
		showProject?: boolean;
		failed?: boolean;
		retry: () => void;
		implement: (t: Task) => Promise<ActionOutcome>;
		retryRun: (t: Task) => Promise<ActionOutcome>;
		stop: (t: Task) => Promise<ActionOutcome>;
		answerPlan: (t: Task, action: 'approve' | 'revise', text?: string) => Promise<ActionOutcome>;
		mergePull: (t: Task) => Promise<MergeOutcome>;
	} = $props();

	let busy = $state<string | null>(null);
	let message = $state('');
	/** A finished action, shown in the detail pane until another task is selected. */
	let done = $state<{ message: string; workflow: string; project: string; runId: string } | null>(
		null
	);

	/** The Approve & merge steps of the selected pull request, once a merge was tried. */
	let steps = $state<MergeStep[]>([]);

	let shown = untrack(() => selected);
	$effect(() => {
		if (selected === shown) return;
		shown = selected;
		untrack(() => {
			done = null;
			steps = [];
		});
	});

	async function answer(t: Task, action: 'approve' | 'revise', text?: string) {
		const r = await answerPlan(t, action, text);
		if (r.ok && 'runId' in t)
			done = {
				message: r.message,
				workflow: t.kind === 'wait' ? t.workflow : '',
				project: t.project,
				runId: t.runId
			};
		return r;
	}

	async function merge(t: Task) {
		busy = t.key;
		message = '';
		done = null;
		steps = [];
		try {
			const r = await mergePull(t);
			steps = r.steps;
			if (r.ok) done = { message: r.message, workflow: '', project: t.project, runId: '' };
			else message = r.message;
		} catch {
			message = 'That did not work. Try again.';
		} finally {
			busy = null;
		}
	}

	async function act(t: Task, id: DefaultActionId) {
		if (id === 'merge') return merge(t);
		const run =
			id === 'implement' ? implement : id === 'retry' ? retryRun : id === 'stop' ? stop : null;
		if (!run) return;
		busy = t.key;
		message = '';
		done = null;
		try {
			const r = await run(t);
			if (r.ok) {
				const workflow =
					id === 'implement' ? IMPLEMENT_WORKFLOW : 'workflow' in t ? t.workflow : '';
				const runId = r.runId ?? ('runId' in t && id === 'stop' ? t.runId : null);
				done = { message: r.message, workflow, project: t.project, runId: runId ?? '' };
			} else message = r.message;
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
		<div class="ez-tasks" data-selected={current || done ? 'true' : 'false'}>
			<TaskSidebar {groups} selected={current?.key ?? null} {href} {showProject} {context} />
			<div class="ez-tasks__pane">
				{#if current || done}
					<a class="ez-tasks__back" href={backHref}>All tasks</a>
				{/if}
				{#if done}
					<p class="ez-tasks__done" role="status" data-testid="task-done">
						{done.message}
						{#if done.runId}
							<RunLink
								workflow={done.workflow}
								id={done.runId}
								href="/projects/{encodeURIComponent(done.project)}/runs/{done.runId}"
							/>
						{/if}
					</p>
					{#if steps.length && !current}<MergeSteps {steps} />{/if}
				{/if}
				{#if current}
					<TaskDetail
						task={current}
						{run}
						{context}
						busy={busy === current.key}
						plan={!!plan}
						act={(id) => act(current, id)}
					>
						{#snippet sections()}
							{#if plan && current.kind === 'wait'}
								<PlanApproval html={plan.html} answer={(a, text) => answer(current, a, text)} />
							{:else if current.kind === 'review'}
								<PullReviewView
									{review}
									project={current.project}
									url={current.githubUrl ??
										`https://github.com/${current.repo}/pull/${current.number}`}
									{steps}
									merging={busy === current.key}
								/>
							{/if}
						{/snippet}
					</TaskDetail>
				{:else if !done}
					<p class="ez-muted ez-tasks__none">Select a task to see it here.</p>
				{/if}
			</div>
		</div>
	{/if}
{/if}

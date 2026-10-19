<script lang="ts">
	import { page } from '$app/state';
	import ArtifactViewer from '#lib/components/tasks/ArtifactViewer.svelte';
	import StepRuns from '#lib/components/tasks/StepRuns.svelte';
	import SubjectLinks from '#lib/components/tasks/SubjectLinks.svelte';
	import TaskCommands from '#lib/components/tasks/TaskCommands.svelte';
	import TaskFamily from '#lib/components/tasks/TaskFamily.svelte';
	import TaskFindings from '#lib/components/tasks/TaskFindings.svelte';
	import TaskControls from '#lib/components/tasks/TaskControls.svelte';
	import TaskStatus from '#lib/components/tasks/TaskStatus.svelte';
	import TaskTimeline from '#lib/components/tasks/TaskTimeline.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getNeedsYou } from '#lib/needs-you.svelte.ts';
	import { getSession, getTaskDetail, submitTask } from '#lib/tasks.remote.ts';
	import { formatCost } from '#lib/usage.ts';

	const id = $derived.by(() => {
		const n = Number(page.params.id);
		return Number.isSafeInteger(n) && n > 0 ? n : null;
	});
	const detail = $derived(id === null ? null : getTaskDetail(id));
	const needsYou = getNeedsYou();

	$effect(() => {
		if (id === null || detail === null) return;
		const d = detail;
		const mine = id;
		return needsYou.onChange((changed) => {
			if (changed === null || changed === mine) void d.refresh();
		});
	});

	/** Always a fresh read: the live tail asks again with the same `afterSeq` until events arrive. */
	async function readSession(taskId: number, runId: string, afterSeq: number) {
		const q = getSession({ taskId, runId, afterSeq });
		await q.refresh();
		return q.current ?? [];
	}

	async function submit(kind: 'stop' | 'resume' | 'cancel' | 'steer' | 'comment', body?: string) {
		if (id === null) return;
		if (kind === 'steer' || kind === 'comment')
			await submitTask({ id, kind, data: { body: body ?? '' } });
		else await submitTask({ id, kind });
	}
</script>

<svelte:head><title>Task {page.params.id} · enzarb</title></svelte:head>

<svelte:boundary>
	{#if id === null}
		<PageHeader title="Task not found" />
		<div><LinkButton href="/tasks">Back to tasks</LinkButton></div>
	{:else if detail?.current}
		{@const t = detail.current}
		<PageHeader title={t.title}>
			{#snippet meta()}
				<TaskStatus status={t.status} endStatus={t.endStatus} />
				<span class="ez-muted">project <a href="/projects/{t.project}/tasks">{t.project}</a></span>
				{#if t.workflowRef}<code>{t.workflowRef}</code>{/if}
				{#if t.phase || t.node}<span class="ez-muted">at {t.phase}/{t.node}</span>{/if}
				{#if t.column}<span class="ez-muted">column {t.column}</span>{/if}
				{#if t.costUsd !== null}<span class="ez-muted">{formatCost(t.costUsd)}</span>{/if}
			{/snippet}
			{#snippet actions()}
				<TaskCommands
					commands={t.commands}
					actions={t.actions}
					oncommand={(command) => submitTask({ id: t.id, kind: 'command', data: { command } })}
				/>
			{/snippet}
		</PageHeader>
		{#if t.reason || t.text}
			<div class="ez-wait-banner" data-testid="wait-reason">
				{#if t.reason}<strong>{t.reason}</strong>{/if}
				{#if t.text}<p>{t.text}</p>{/if}
				{#if t.waitingOn}<span class="ez-muted">waiting on {t.waitingOn}</span>{/if}
			</div>
		{/if}

		{#if t.findings.length}
			<Section title="Findings">
				<TaskFindings findings={t.findings} />
			</Section>
		{/if}

		{#if t.parent || t.children.length}
			<Section
				title={t.parent && t.children.length
					? 'Epic and sub-issues'
					: t.children.length
						? 'Sub-issues'
						: 'Epic'}
			>
				<TaskFamily parent={t.parent} subIssues={t.children} />
			</Section>
		{/if}

		<Section title="Subjects">
			<SubjectLinks subjects={t.subjects} />
			{#if !t.subjects.length}<p class="ez-muted">No linked subjects.</p>{/if}
		</Section>
		<Section title="Artifacts">
			<ArtifactViewer artifacts={t.artifacts} />
		</Section>
		<Section title="Steer, comment or control">
			<TaskControls status={t.status} onsubmit={submit} />
		</Section>
		<Section title="Steps">
			<StepRuns runs={t.stepRuns} load={(runId, afterSeq) => readSession(t.id, runId, afterSeq)} />
		</Section>
		<Section title="Timeline">
			<TaskTimeline transitions={t.transitions} />
		</Section>
	{:else}
		<Skeleton label="Loading task" rows={4} />
	{/if}
	{#snippet failed()}
		<PageHeader title="Task not found" />
		<div><LinkButton href="/tasks">Back to tasks</LinkButton></div>
	{/snippet}
</svelte:boundary>

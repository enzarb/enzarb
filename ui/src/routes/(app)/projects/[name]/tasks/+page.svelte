<script lang="ts">
	import { page } from '$app/state';
	import TaskList from '#lib/components/TaskList.svelte';
	import { getRun } from '#lib/runs.remote.ts';
	import { taskActions } from '#lib/task-actions.ts';
	import { listAllTasks, listProjectTasks, taskCount } from '#lib/tasks.remote.ts';
	import { runOfTask } from '#lib/tasks.ts';

	const name = $derived(page.params.name!);
	const tasks = $derived(listProjectTasks({ name }));
	const actions = $derived(taskActions(() => [tasks, listAllTasks(), taskCount()]));
	const selected = $derived(page.url.searchParams.get('task'));
	const ref = $derived(runOfTask(tasks.current, selected));
	const runQuery = $derived(ref ? getRun({ name: ref.project, id: ref.id }) : null);
	const run = $derived(
		runQuery ? (runQuery.current ?? (runQuery.error ? null : undefined)) : undefined
	);
</script>

<svelte:head><title>Tasks · {page.params.name} · enzarb</title></svelte:head>

<TaskList
	list={tasks.current}
	{selected}
	{run}
	href={(key) => `${page.url.pathname}?task=${encodeURIComponent(key)}`}
	backHref={page.url.pathname}
	failed={!!tasks.error && !tasks.current}
	retry={() => void tasks.refresh()}
	{...actions}
/>

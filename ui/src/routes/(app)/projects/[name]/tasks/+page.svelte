<script lang="ts">
	import { page } from '$app/state';
	import TaskList from '#lib/components/TaskList.svelte';
	import { taskActions } from '#lib/task-actions.ts';
	import { listAllTasks, listProjectTasks, taskCount } from '#lib/tasks.remote.ts';

	const name = $derived(page.params.name!);
	const tasks = $derived(listProjectTasks({ name }));
	const actions = $derived(taskActions(() => [tasks, listAllTasks(), taskCount()]));
</script>

<svelte:head><title>Tasks · {page.params.name} · enzarb</title></svelte:head>

<TaskList
	list={tasks.current}
	failed={!!tasks.error && !tasks.current}
	retry={() => void tasks.refresh()}
	{...actions}
/>

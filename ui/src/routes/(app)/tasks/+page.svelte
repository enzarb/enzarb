<script lang="ts">
	import TaskList from '#lib/components/TaskList.svelte';
	import PageHeader from '#lib/components/ui/PageHeader.svelte';
	import { taskActions } from '#lib/task-actions.ts';
	import { listAllTasks, taskCount } from '#lib/tasks.remote.ts';

	const tasks = listAllTasks();
	const actions = taskActions(() => [tasks, taskCount()]);
</script>

<svelte:head><title>Tasks · enzarb</title></svelte:head>

<PageHeader title="Tasks" />
<TaskList
	list={tasks.current}
	showProject
	failed={!!tasks.error && !tasks.current}
	retry={() => void tasks.refresh()}
	{...actions}
/>

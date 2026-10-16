<script lang="ts">
	import { page } from '$app/state';
	import TaskBoard from '#lib/components/tasks/TaskBoard.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import Skeleton from '#lib/components/ui/Skeleton.svelte';
	import { getNeedsYou } from '#lib/needs-you.svelte.ts';
	import { applyRow } from '#lib/tasks.ts';
	import { getBoard, getTask, listTasks, submitTask } from '#lib/tasks.remote.ts';

	const name = $derived(page.params.name!);
	const board = $derived(getBoard(name));
	const tasks = $derived(listTasks(name));
	const needsYou = getNeedsYou();
	const highlight = $derived(Number(page.url.searchParams.get('highlight')) || null);

	/** Patch one changed row in place; any failure reloads the list. */
	async function applyChange(id: number) {
		const q = tasks;
		try {
			const row = await getTask(id);
			if (q !== tasks || !q.current) return void q.refresh();
			q.set(applyRow(q.current, id, row, (t) => t.project === name));
		} catch {
			void q.refresh();
		}
	}

	// The app layout owns the live socket; this page only listens to it.
	$effect(() => needsYou.onChange((id) => void (id === null ? tasks.refresh() : applyChange(id))));
</script>

<svelte:head><title>Tasks · {name} · enzarb</title></svelte:head>

{#if (board.error && !board.current) || (tasks.error && !tasks.current)}
	<Notice tone="error">Could not load the board.</Notice>
{:else if !board.current || !tasks.current}
	<Skeleton label="Loading the board" rows={4} />
{:else}
	<TaskBoard
		board={board.current}
		tasks={tasks.current}
		{highlight}
		oncommand={(id, command) => submitTask({ id, kind: 'command', data: { command } })}
	/>
{/if}

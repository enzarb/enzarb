<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import type { UiAction } from '#lib/tasks.ts';

	/** The buttons the current wait declares; each posts its `/command` (submit kind `command`). */
	let {
		actions,
		oncommand
	}: { actions: UiAction[]; oncommand: (command: string) => Promise<unknown> } = $props();

	let pending = $state<string | null>(null);
	let failed = $state('');

	async function run(command: string) {
		pending = command;
		failed = '';
		try {
			await oncommand(command);
		} catch {
			failed = `Could not send /${command}.`;
		} finally {
			pending = null;
		}
	}
</script>

{#if actions.length}
	<div class="ez-cluster" role="group" aria-label="Actions">
		{#each actions as a, i (a.command + i)}
			<Button
				variant={i === 0 ? 'primary' : 'secondary'}
				size="sm"
				disabled={pending !== null}
				loading={pending === a.command}
				onclick={() => run(a.command)}>{a.label}</Button
			>
		{/each}
	</div>
	{#if failed}<p class="ez-task-error" role="alert">{failed}</p>{/if}
{/if}

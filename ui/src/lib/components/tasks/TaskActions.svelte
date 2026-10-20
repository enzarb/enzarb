<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import { safeLink, type UiAction } from '#lib/tasks.ts';

	/**
	 * The buttons the current wait declares: a command button posts its `/command` (submit kind
	 * `command`), a link button opens its page in a new tab.
	 */
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
		{#each actions as a, i ((a.command ?? a.link) + String(i))}
			{#if safeLink(a)}
				<LinkButton
					variant={i === 0 ? 'primary' : 'secondary'}
					size="sm"
					href={safeLink(a)!}
					target="_blank"
					rel="noopener noreferrer">{a.label}</LinkButton
				>
			{:else if a.command}
				{@const command = a.command}
				<Button
					variant={i === 0 ? 'primary' : 'secondary'}
					size="sm"
					disabled={pending !== null}
					loading={pending === command}
					onclick={() => run(command)}>{a.label}</Button
				>
			{/if}
		{/each}
	</div>
	{#if failed}<p class="ez-task-error" role="alert">{failed}</p>{/if}
{/if}

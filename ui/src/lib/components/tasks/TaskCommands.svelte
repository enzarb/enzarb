<script lang="ts">
	import Button from '#lib/components/form/Button.svelte';
	import LinkButton from '#lib/components/ui/LinkButton.svelte';
	import { safeLink, type TaskCommand, type UiAction } from '#lib/tasks.ts';

	/**
	 * The commands that apply to the task now (`task.commands`, from the engine's projection), as
	 * buttons; each posts `/name` as the user (submit kind `command`). A command that needs write
	 * access is hidden from a viewer without it. `/cancel` has its own confirmed button in the task
	 * controls and is left out here. An older engine sends no `commands`: its declared `actions`
	 * are shown instead. The wait's own declared button is the primary one.
	 */
	let {
		commands,
		actions = [],
		canWrite = true,
		oncommand
	}: {
		commands: TaskCommand[];
		actions?: UiAction[];
		canWrite?: boolean;
		oncommand: (command: string) => Promise<unknown>;
	} = $props();

	const links = $derived(actions.filter((a) => safeLink(a)));
	const declared = $derived(new Set(actions.flatMap((a) => (a.command ? [a.command] : []))));
	const items = $derived(
		commands.length
			? commands.filter((c) => c.name !== 'cancel' && (c.from === 'anyone' || canWrite))
			: actions.flatMap((a) =>
					a.command
						? [{ name: a.command, label: a.label, description: null, from: 'anyone' as const }]
						: []
				)
	);

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

{#if items.length || links.length}
	<div class="ez-cluster" role="group" aria-label="Commands">
		{#each links as a, i (String(i) + a.link)}
			<LinkButton
				variant="primary"
				size="sm"
				href={safeLink(a)!}
				target="_blank"
				rel="noopener noreferrer">{a.label}</LinkButton
			>
		{/each}
		{#each items as c (c.name)}
			<Button
				variant={declared.has(c.name) ? 'primary' : 'secondary'}
				size="sm"
				title={c.description ?? undefined}
				disabled={pending !== null}
				loading={pending === c.name}
				onclick={() => run(c.name)}>{c.label}</Button
			>
		{/each}
	</div>
	{#if failed}<p class="ez-task-error" role="alert">{failed}</p>{/if}
{/if}

<script lang="ts">
	import Button from './form/Button.svelte';
	import type { WaitButton } from '#lib/work-items.ts';

	/**
	 * The buttons a graph wait declares (`state_data.wait.actions`, ADR-0020). A click hands the
	 * button's command to `oncommand`, which posts the comment `/<command>`; the engine checks
	 * write access like for any comment.
	 */
	let {
		buttons,
		disabled = false,
		oncommand
	}: {
		buttons: WaitButton[];
		disabled?: boolean;
		oncommand: (command: string) => void;
	} = $props();
</script>

{#if buttons.length}
	<div class="ez-cluster">
		{#each buttons as b, i (b.command)}
			<Button
				variant={i === 0 ? 'primary' : 'secondary'}
				{disabled}
				title="Posts /{b.command} as you"
				onclick={() => oncommand(b.command)}
			>
				{b.label}
			</Button>
		{/each}
	</div>
{/if}

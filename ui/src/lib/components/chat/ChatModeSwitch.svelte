<script lang="ts">
	import { offeredModes } from '#lib/acp/reducer.ts';
	import type { ModeId, ModeState } from '#lib/acp/types.ts';

	let {
		mode,
		disabled = false,
		onSelect
	}: {
		mode: ModeState;
		/** Not ready, or a change is already in flight. */
		disabled?: boolean;
		onSelect: (modeId: ModeId) => void;
	} = $props();

	const LABELS: Record<ModeId, string> = {
		bypassPermissions: 'Bypass',
		default: 'Default',
		plan: 'Plan'
	};

	let confirming = $state(false);
	// The active mode is always listed so the select can show it, even when it is not switchable.
	const options = $derived.by(() => {
		const offered = offeredModes(mode);
		return offered.includes(mode.modeId) ? offered : [mode.modeId, ...offered];
	});

	function onChange(e: Event & { currentTarget: HTMLSelectElement }) {
		const id = e.currentTarget.value as ModeId;
		// The shown mode only follows the sandbox's confirmation.
		e.currentTarget.value = mode.modeId;
		if (id === mode.modeId) return;
		if (id === 'bypassPermissions') confirming = true;
		else onSelect(id);
	}
</script>

<span class="ez-chat-mode-switch">
	<select
		class="ez-control ez-chat-mode-select"
		aria-label="Permission mode"
		value={mode.modeId}
		{disabled}
		onchange={onChange}
	>
		{#each options as id (id)}
			<option value={id} disabled={!offeredModes(mode).includes(id)}>{LABELS[id]}</option>
		{/each}
	</select>
	{#if confirming}
		<span class="ez-chat-mode-confirm" role="group" aria-label="Confirm bypass">
			<span>Turn off permission prompts?</span>
			<button
				type="button"
				class="ez-btn ez-btn--secondary ez-btn--sm"
				{disabled}
				onclick={() => {
					confirming = false;
					onSelect('bypassPermissions');
				}}>Confirm</button
			>
			<button
				type="button"
				class="ez-btn ez-btn--secondary ez-btn--sm"
				onclick={() => (confirming = false)}>Cancel</button
			>
		</span>
	{/if}
</span>

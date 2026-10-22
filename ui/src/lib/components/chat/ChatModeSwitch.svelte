<script lang="ts">
	import { offeredModes } from '#lib/acp/reducer.ts';
	import { BYPASS_MODE, type ModeId, type ModeOption, type ModeState } from '#lib/acp/types.ts';

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

	let confirming = $state(false);
	const offered = $derived(offeredModes(mode));
	// The active mode is always listed so the select can show it, even when it is not switchable.
	const options = $derived.by((): ModeOption[] => {
		if (offered.some((m) => m.id === mode.modeId)) return offered;
		const current = mode.available?.find((m) => m.id === mode.modeId) ?? {
			id: mode.modeId,
			name: mode.modeId
		};
		return [current, ...offered];
	});
	const active = $derived(options.find((m) => m.id === mode.modeId));

	function onChange(e: Event & { currentTarget: HTMLSelectElement }) {
		const id = e.currentTarget.value;
		// The shown mode only follows the sandbox's confirmation.
		e.currentTarget.value = mode.modeId;
		if (id === mode.modeId) return;
		if (id === BYPASS_MODE) confirming = true;
		else onSelect(id);
	}
</script>

<span class="ez-chat-mode-switch">
	<select
		class="ez-control ez-chat-mode-select"
		aria-label="Mode"
		title={active?.description}
		value={mode.modeId}
		{disabled}
		onchange={onChange}
	>
		{#each options as m (m.id)}
			<option value={m.id} title={m.description} disabled={!offered.some((o) => o.id === m.id)}
				>{m.name}</option
			>
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
					onSelect(BYPASS_MODE);
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

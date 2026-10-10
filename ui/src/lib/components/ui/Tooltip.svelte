<script lang="ts">
	import { Tooltip } from 'bits-ui';
	import type { Snippet } from 'svelte';

	/**
	 * Short hint on hover and keyboard focus (also announced as the trigger's description).
	 * `trigger` receives the props to spread on the element that owns the tooltip, which must be
	 * focusable (a button or link). Never put essential information only in a tooltip.
	 */
	let {
		text,
		trigger,
		side = 'top'
	}: {
		text: string;
		trigger: Snippet<[Record<string, unknown>]>;
		side?: 'top' | 'bottom' | 'left' | 'right';
	} = $props();
</script>

<Tooltip.Provider delayDuration={300}>
	<Tooltip.Root>
		<Tooltip.Trigger>
			{#snippet child({ props })}{@render trigger(props)}{/snippet}
		</Tooltip.Trigger>
		<Tooltip.Portal>
			<Tooltip.Content class="ez-tooltip" {side} sideOffset={6}>{text}</Tooltip.Content>
		</Tooltip.Portal>
	</Tooltip.Root>
</Tooltip.Provider>

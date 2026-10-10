<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { Tone } from './status.ts';

	/**
	 * Small status label: colour plus a dot, and always the text, so state never relies on colour
	 * alone. One component for work states, Project conditions and run results; the domain
	 * wrappers (StatusBadge, WorkStateChip, JobTable) only map a value to a tone.
	 * `hollow` draws an outlined dot ("waiting"); `plain` drops the dot.
	 */
	let {
		tone = 'neutral',
		hollow = false,
		plain = false,
		children
	}: { tone?: Tone; hollow?: boolean; plain?: boolean; children: Snippet } = $props();
</script>

<span
	class="ez-badge"
	class:ez-badge--accent={tone === 'accent'}
	class:ez-badge--success={tone === 'success'}
	class:ez-badge--warning={tone === 'warning'}
	class:ez-badge--danger={tone === 'danger'}
	class:ez-badge--info={tone === 'info'}
	class:ez-badge--hollow={hollow}
	class:ez-badge--plain={plain}
>
	{@render children()}
</span>

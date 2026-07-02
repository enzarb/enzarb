<script lang="ts">
	import type { DiffPayload } from './types';
	import DiffView from './DiffView.svelte';

	let {
		toolKind,
		title,
		status,
		diff
	}: { toolKind: string; title: string; status: string; diff: DiffPayload | null } = $props();

	const ICONS: Record<string, string> = {
		read: '👁',
		edit: '✏️',
		execute: '▶',
		other: '🔧'
	};
</script>

<div class="tool-card {status}">
	<div class="tool-header">
		<span class="tool-icon">{ICONS[toolKind] ?? ICONS.other}</span>
		<span class="tool-title">{title}</span>
		<span class="tool-status">{status}</span>
	</div>
	{#if diff}
		<DiffView {diff} />
	{/if}
</div>

<style>
	.tool-card { border: 1px solid var(--color-border); border-radius: 6px; padding: 0.5rem 0.7rem; font-size: 12px; }
	.tool-card.failed { border-color: var(--color-danger); }
	.tool-header { display: flex; align-items: center; gap: 0.4rem; }
	.tool-icon { flex-shrink: 0; }
	.tool-title { flex: 1; font-family: var(--font-mono); }
	.tool-status { color: var(--color-text-muted); text-transform: capitalize; font-size: 11px; }
	.tool-card :global(.diff-view) { margin-top: 0.5rem; }
</style>

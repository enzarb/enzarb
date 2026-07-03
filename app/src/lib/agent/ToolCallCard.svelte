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

	let expanded = $state(false);
	const hasBody = $derived(!!diff);
</script>

<div class="tool-card {status}" class:expanded>
	<button
		class="tool-header"
		type="button"
		onclick={() => { if (hasBody) expanded = !expanded; }}
		disabled={!hasBody}
	>
		<span class="tool-icon">{ICONS[toolKind] ?? ICONS.other}</span>
		<span class="tool-title">{title}</span>
		<span class="tool-status">{status}</span>
		{#if hasBody}
			<span class="tool-chevron" class:open={expanded}>▸</span>
		{/if}
	</button>
	{#if expanded && diff}
		<div class="tool-body">
			<DiffView {diff} />
		</div>
	{/if}
</div>

<style>
	.tool-card { border: 1px solid var(--color-border); border-radius: 6px; font-size: 12px; }
	.tool-card.failed { border-color: var(--color-danger); }
	.tool-header {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		width: 100%;
		padding: 0.5rem 0.7rem;
		background: none;
		border: none;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: default;
	}
	.tool-header:not(:disabled) { cursor: pointer; }
	.tool-header:not(:disabled):hover { background: var(--color-surface-2); border-radius: 6px; }
	.expanded .tool-header:not(:disabled):hover { border-radius: 6px 6px 0 0; }
	.tool-icon { flex-shrink: 0; }
	.tool-title { flex: 1; font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.tool-status { color: var(--color-text-muted); text-transform: capitalize; font-size: 11px; flex-shrink: 0; }
	.tool-chevron { color: var(--color-text-muted); flex-shrink: 0; transition: transform 0.15s; display: inline-block; }
	.tool-chevron.open { transform: rotate(90deg); }
	.tool-body { padding: 0 0.7rem 0.5rem; border-top: 1px solid var(--color-border); }
	.tool-body :global(.diff-view) { margin-top: 0.5rem; }
</style>

<script lang="ts">
	import type { DiffPayload } from './types';
	import DiffView from './DiffView.svelte';
	import Markdown from './Markdown.svelte';

	let {
		toolKind,
		title,
		status,
		diff,
		output = null,
		plan = null
	}: { toolKind: string; title: string; status: string; diff: DiffPayload | null; output?: string | null; plan?: string | null } = $props();

	const ICONS: Record<string, string> = {
		read: '👁',
		edit: '✏️',
		execute: '▶',
		other: '🔧'
	};

	// A plan is the thing the user is being asked to review — open by default.
	let expanded = $state(!!plan);
	const hasBody = $derived(!!diff || !!output || !!plan);
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
	{#if expanded}
		<div class="tool-body">
			{#if plan}
				<div class="tool-plan"><Markdown text={plan} /></div>
			{:else if diff}
				<DiffView {diff} />
			{:else if output}
				<pre class="tool-output">{output}</pre>
			{/if}
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
	.tool-plan { margin-top: 0.5rem; font-size: 12px; line-height: 1.5; overflow-y: auto; max-height: 480px; }
	.tool-output {
		margin: 0.5rem 0 0;
		padding: 0.5rem 0.6rem;
		background: var(--color-bg);
		border-radius: 4px;
		font-family: var(--font-mono);
		font-size: 11px;
		line-height: 1.45;
		white-space: pre-wrap;
		word-break: break-word;
		overflow-y: auto;
		max-height: 320px;
	}
</style>

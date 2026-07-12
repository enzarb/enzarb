<script lang="ts">
	import { untrack } from 'svelte';
	import type { DiffPayload } from './types';
	import DiffView from './DiffView.svelte';
	import Markdown from './Markdown.svelte';

	let {
		toolKind,
		title,
		status,
		path = null,
		diff,
		output = null,
		plan = null,
		command = null,
		input = null
	}: { toolKind: string; title: string; status: string; path?: string | null; diff: DiffPayload | null; output?: string | null; plan?: string | null; command?: string | null; input?: unknown } = $props();

	const ICONS: Record<string, string> = {
		read: '👁',
		edit: '✏️',
		execute: '▶',
		other: '🔧'
	};

	// A plan is the thing the user is being asked to review — open by default,
	// including when it arrives later via a tool_call_updated event rather
	// than being present on the initial render. untrack() marks these as
	// deliberately one-time reads of the initial prop value, not a missed
	// reactive dependency.
	let expanded = $state(untrack(() => !!plan));
	let autoOpened = untrack(() => !!plan);
	$effect(() => {
		if (plan && !autoOpened) {
			autoOpened = true;
			expanded = true;
		}
	});

	// A plain-object view of the raw tool input, or null when there's nothing
	// useful to show (missing, or a non-object primitive other than a string —
	// those are handled via inputString below).
	const inputObj = $derived(
		input && typeof input === 'object' && !Array.isArray(input)
			? (input as Record<string, unknown>)
			: null
	);
	const inputString = $derived(typeof input === 'string' ? input : null);
	const hasInput = $derived(!!inputObj || inputString !== null);

	// Fields worth summarizing in the collapsed header, in priority order — the
	// first one present becomes the one-line "what is this tool doing" snippet.
	const SUMMARY_FIELDS = ['query', 'pattern', 'path', 'url', 'prompt', 'command'];
	const inputSummary = $derived.by(() => {
		if (inputString !== null) return inputString;
		if (!inputObj) return null;
		for (const f of SUMMARY_FIELDS) {
			const v = inputObj[f];
			if (typeof v === 'string' && v.length) return v;
			if (typeof v === 'number' || typeof v === 'boolean') return String(v);
		}
		return null;
	});

	// path is already surfaced separately in the header, so don't repeat it as
	// the summary snippet when it's the only thing we'd have shown.
	const showSummary = $derived(!!inputSummary && !(path && inputSummary === path));

	// Pretty key/value rows for the expanded Input section. Skip huge/blob
	// values (they're rarely the point and blow up the card) and cap length.
	const MAX_VAL = 300;
	const inputRows = $derived.by(() => {
		if (!inputObj) return [];
		return Object.entries(inputObj)
			.map(([k, v]) => {
				if (v == null) return null;
				let s = typeof v === 'string' ? v : JSON.stringify(v);
				if (s == null) return null;
				if (s.length > MAX_VAL) s = s.slice(0, MAX_VAL) + '…';
				return { key: k, value: s };
			})
			.filter((r): r is { key: string; value: string } => r !== null);
	});

	const inputJson = $derived(inputObj ? JSON.stringify(inputObj, null, 2) : '');

	const hasBody = $derived(!!diff || !!output || !!plan || hasInput);

	async function copy(text: string) {
		try {
			await navigator.clipboard.writeText(text);
		} catch {}
	}
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
		{#if path}
			<span class="tool-path">{path}</span>
		{:else if showSummary}
			<span class="tool-path">{inputSummary}</span>
		{/if}
		<span class="tool-status">{status}</span>
		{#if hasBody}
			<span class="tool-chevron" class:open={expanded}>▸</span>
		{/if}
	</button>
	{#if command && (status === 'pending' || status === 'running')}
		<div class="tool-command"><code>{command}</code></div>
	{/if}
	{#if expanded}
		<div class="tool-body">
			{#if hasInput}
				<div class="tool-section">
					<div class="tool-section-head">
						<span class="tool-section-label">Input</span>
						<button
							class="btn btn-icon btn-xs"
							type="button"
							title="Copy input"
							onclick={() => copy(inputString ?? inputJson)}
						>⧉</button>
					</div>
					{#if inputString !== null}
						<div class="tool-input"><code>{inputString}</code></div>
					{:else if inputRows.length}
						<dl class="tool-input-kv">
							{#each inputRows as row (row.key)}
								<dt>{row.key}</dt>
								<dd>{row.value}</dd>
							{/each}
						</dl>
					{:else}
						<div class="tool-input"><code>{inputJson}</code></div>
					{/if}
				</div>
			{/if}
			{#if plan}
				<div class="tool-plan"><Markdown text={plan} /></div>
			{:else if diff}
				<DiffView {diff} />
			{:else if output}
				<div class="tool-section">
					<div class="tool-section-head">
						<span class="tool-section-label">Output</span>
						<button
							class="btn btn-icon btn-xs"
							type="button"
							title="Copy output"
							onclick={() => copy(output ?? '')}
						>⧉</button>
					</div>
					<div class="tool-output"><Markdown text={output} /></div>
				</div>
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
	.tool-title { flex: 0 1 auto; min-width: 0; font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.tool-path {
		flex: 1 1 auto;
		min-width: 0;
		color: var(--color-text-muted);
		font-family: var(--font-mono);
		font-size: 11px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.tool-status { color: var(--color-text-muted); text-transform: capitalize; font-size: 11px; flex-shrink: 0; }
	.tool-chevron { color: var(--color-text-muted); flex-shrink: 0; transition: transform 0.15s; display: inline-block; }
	.tool-chevron.open { transform: rotate(90deg); }
	.tool-command {
		margin: 0 0.7rem 0.5rem;
		padding: 0.35rem 0.5rem;
		background: var(--color-bg);
		border-radius: 4px;
		font-family: var(--font-mono);
		font-size: 11px;
		overflow-x: auto;
		white-space: pre;
	}
	.tool-body { padding: 0 0.7rem 0.5rem; border-top: 1px solid var(--color-border); }
	.tool-section { margin-top: 0.5rem; }
	.tool-section-head { display: flex; align-items: center; justify-content: space-between; gap: 0.4rem; }
	.tool-section-label { font-size: 10px; text-transform: uppercase; letter-spacing: 0.04em; color: var(--color-text-muted); }
	.tool-input {
		margin: 0.3rem 0 0;
		padding: 0.5rem 0.6rem;
		background: var(--color-bg);
		border-radius: 4px;
		font-family: var(--font-mono);
		font-size: 11px;
		line-height: 1.45;
		overflow: auto;
		max-height: 320px;
	}
	.tool-input code { white-space: pre-wrap; overflow-wrap: anywhere; }
	.tool-input-kv {
		display: grid;
		grid-template-columns: minmax(0, max-content) 1fr;
		gap: 0.15rem 0.6rem;
		margin: 0.3rem 0 0;
		padding: 0.5rem 0.6rem;
		background: var(--color-bg);
		border-radius: 4px;
		font-size: 11px;
	}
	.tool-input-kv dt { color: var(--color-text-muted); font-family: var(--font-mono); }
	.tool-input-kv dd { margin: 0; font-family: var(--font-mono); overflow-wrap: anywhere; white-space: pre-wrap; }
	.tool-body :global(.diff-view) { margin-top: 0.5rem; }
	.tool-plan { margin-top: 0.5rem; font-size: 12px; line-height: 1.5; overflow-y: auto; max-height: 480px; }
	.tool-output {
		margin: 0.3rem 0 0;
		padding: 0.5rem 0.6rem;
		background: var(--color-bg);
		border-radius: 4px;
		font-size: 11px;
		line-height: 1.45;
		overflow-y: auto;
		max-height: 320px;
	}
	.tool-output :global(.md-code) { font-size: 11px; }
</style>

<script lang="ts">
	import type { DiffPayload } from './types';
	import { diffLines } from './lineDiff';

	let { diff }: { diff: DiffPayload } = $props();
	let lines = $derived(diffLines(diff.old_text, diff.new_text));
</script>

<div class="diff-view">
	<div class="diff-path">{diff.path}</div>
	<pre class="diff-body"><code
		>{#each lines as line, i (i)}<span class="diff-line {line.kind}"
			>{line.kind === 'add' ? '+' : line.kind === 'remove' ? '-' : ' '}{line.text}</span
		>{'\n'}{/each}</code
	></pre>
</div>

<style>
	.diff-view {
		border: 1px solid var(--color-border);
		border-radius: 4px;
		overflow: hidden;
		font-size: 12px;
	}
	.diff-path {
		padding: 0.3rem 0.6rem;
		background: var(--color-surface-2);
		border-bottom: 1px solid var(--color-border);
		font-family: var(--font-mono);
		color: var(--color-text-muted);
	}
	.diff-body {
		margin: 0;
		padding: 0.4rem 0;
		overflow-x: auto;
		background: var(--color-surface);
	}
	.diff-line {
		display: block;
		white-space: pre;
		font-family: var(--font-mono);
		padding: 0 0.6rem;
	}
	.diff-line.add {
		background: color-mix(in srgb, #3fb950 15%, transparent);
		color: #3fb950;
	}
	.diff-line.remove {
		background: color-mix(in srgb, var(--color-danger) 15%, transparent);
		color: var(--color-danger);
	}
</style>

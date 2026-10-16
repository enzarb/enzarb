<script lang="ts">
	import type { Artifact } from '#lib/tasks.ts';

	/**
	 * A task's artifacts, rendered by format: markdown as server-sanitized HTML, json pretty-printed,
	 * anything else as text. Each has an anchor (`#artifact-<i>`) that cards link to.
	 */
	let { artifacts }: { artifacts: Artifact[] } = $props();

	function pretty(content: string): string {
		try {
			return JSON.stringify(JSON.parse(content), null, 2);
		} catch {
			return content;
		}
	}
</script>

{#each artifacts as a, i (i)}
	<details class="ez-artifact" id="artifact-{i}" open={i === artifacts.length - 1}>
		<summary>
			<strong>{a.title}</strong>
			<span class="ez-muted">{a.format}{a.node ? ` · ${a.node}` : ''}</span>
		</summary>
		{#if a.format === 'markdown' && a.html !== undefined}
			<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized server-side (renderMarkdown) -->
			<div class="ez-markdown" data-testid="artifact-markdown">{@html a.html}</div>
		{:else if a.format === 'json'}
			<pre class="ez-artifact__pre">{pretty(a.content)}</pre>
		{:else}
			<pre class="ez-artifact__pre">{a.content}</pre>
		{/if}
	</details>
{:else}
	<p class="ez-muted">No artifacts yet.</p>
{/each}

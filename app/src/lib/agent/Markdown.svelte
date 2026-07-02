<script lang="ts">
	// Renders markdown by walking marked's token tree into Svelte elements —
	// deliberately no {@html}/innerHTML anywhere, so nothing the agent writes
	// can inject markup into the page.
	import { marked } from 'marked';
	import type { Token, Tokens } from 'marked';
	import Markdown from './Markdown.svelte';
	import MarkdownInline from './MarkdownInline.svelte';

	let { text }: { text: string } = $props();

	let tokens = $derived(marked.lexer(text) as Token[]);
</script>

{#each tokens as token (token.raw)}
	{#if token.type === 'paragraph'}
		<p><MarkdownInline tokens={(token as Tokens.Paragraph).tokens} /></p>
	{:else if token.type === 'heading'}
		{@const h = token as Tokens.Heading}
		{#if h.depth <= 2}<h4><MarkdownInline tokens={h.tokens} /></h4>
		{:else}<h5><MarkdownInline tokens={h.tokens} /></h5>{/if}
	{:else if token.type === 'code'}
		{@const c = token as Tokens.Code}
		<pre class="md-code"><code>{c.text}</code></pre>
	{:else if token.type === 'blockquote'}
		<blockquote><Markdown text={(token as Tokens.Blockquote).text} /></blockquote>
	{:else if token.type === 'list'}
		{@const l = token as Tokens.List}
		{#if l.ordered}
			<ol>
				{#each l.items as item (item.raw)}
					<li><MarkdownInline tokens={item.tokens as Token[]} /></li>
				{/each}
			</ol>
		{:else}
			<ul>
				{#each l.items as item (item.raw)}
					<li><MarkdownInline tokens={item.tokens as Token[]} /></li>
				{/each}
			</ul>
		{/if}
	{:else if token.type === 'hr'}
		<hr />
	{:else if token.type !== 'space'}
		<p><MarkdownInline tokens={[{ type: 'text', raw: token.raw, text: 'raw' in token ? token.raw : '' }]} /></p>
	{/if}
{/each}

<style>
	p, h4, h5, ul, ol, blockquote { margin: 0 0 0.5rem; }
	p:last-child, ul:last-child, ol:last-child { margin-bottom: 0; }
	.md-code {
		background: var(--color-surface-2);
		border: 1px solid var(--color-border);
		border-radius: 4px;
		padding: 0.5rem 0.75rem;
		overflow-x: auto;
		font-family: var(--font-mono);
		font-size: 12px;
	}
	blockquote {
		border-left: 2px solid var(--color-border);
		padding-left: 0.6rem;
		color: var(--color-text-muted);
	}
</style>

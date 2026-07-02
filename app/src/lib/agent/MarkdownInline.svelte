<script lang="ts">
	// Inline-level counterpart to Markdown.svelte — same no-{@html} rule.
	import type { Token, Tokens } from 'marked';
	import MarkdownInline from './MarkdownInline.svelte';
	import { isExternalHttpUrl } from '$lib/terminal/links';

	let { tokens }: { tokens: Token[] } = $props();
</script>

{#each tokens as token, i (i)}
	{#if token.type === 'text' || token.type === 'escape'}
		{(token as Tokens.Text).text}
	{:else if token.type === 'strong'}
		<strong><MarkdownInline tokens={(token as Tokens.Strong).tokens} /></strong>
	{:else if token.type === 'em'}
		<em><MarkdownInline tokens={(token as Tokens.Em).tokens} /></em>
	{:else if token.type === 'codespan'}
		<code>{(token as Tokens.Codespan).text}</code>
	{:else if token.type === 'link'}
		{@const l = token as Tokens.Link}
		{#if isExternalHttpUrl(l.href)}
			<a href={l.href} target="_blank" rel="noopener noreferrer"><MarkdownInline tokens={l.tokens} /></a>
		{:else}
			<MarkdownInline tokens={l.tokens} />
		{/if}
	{:else if token.type === 'br'}
		<br />
	{:else if 'text' in token}
		{(token as { text: string }).text}
	{/if}
{/each}

<script lang="ts">
	import { cacheBreakdown, formatCost, formatTokens, totalInput, type Usage } from '#lib/usage.ts';
	import UsageFigure from './UsageFigure.svelte';

	/**
	 * One job's (or a whole item's) usage on a line: tokens in/out, cache, models and cost. Cost is
	 * as reported by Claude; a dash means it was not reported.
	 */
	let { usage, label }: { usage: Usage | null | undefined; label?: string } = $props();
</script>

{#if usage}
	<p class="ez-usage-line" data-testid="usage">
		{#if label}<strong>{label}</strong>{/if}
		<span>in <UsageFigure value={formatTokens(totalInput(usage))} /></span>
		<span>out <UsageFigure value={formatTokens(usage.outputTokens)} /></span>
		<span>cache <span>{cacheBreakdown(usage)}</span></span>
		{#if usage.models.length}<span><code>{usage.models.join(', ')}</code></span>{/if}
		<span>cost <UsageFigure value={formatCost(usage.costUsd)} /></span>
	</p>
{:else}
	<span class="ez-subtle">{label ? `${label}: ` : ''}—</span>
{/if}

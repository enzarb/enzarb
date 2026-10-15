<script lang="ts">
	import type { UsageTotal } from '#lib/server/metrics-service.ts';
	import { cacheBreakdown, formatCost, formatTokens, totalInput } from '#lib/usage.ts';
	import UsageFigure from './UsageFigure.svelte';

	/** Totals of a window. A figure no task reported is a dash, never 0. */
	let { total }: { total: UsageTotal } = $props();

	const u = $derived(total.usage);
</script>

<dl class="ez-metrics-summary" data-testid="usage-summary">
	<div>
		<dt>Tasks</dt>
		<dd>{total.items}</dd>
	</div>
	<div>
		<dt>Input</dt>
		<dd>
			<UsageFigure value={formatTokens(totalInput(u))} hint={u ? cacheBreakdown(u) : undefined} />
		</dd>
	</div>
	<div>
		<dt>Output</dt>
		<dd><UsageFigure value={formatTokens(u?.outputTokens)} /></dd>
	</div>
	<div>
		<dt>Cost</dt>
		<dd><UsageFigure value={formatCost(u?.costUsd)} /></dd>
	</div>
	<div>
		<dt>Cost per task</dt>
		<dd>
			avg <UsageFigure value={formatCost(total.cost.avg)} /> · median
			<UsageFigure value={formatCost(total.cost.median)} />
		</dd>
	</div>
	<div>
		<dt>Cost reported by</dt>
		<dd>{total.cost.reporters} of {total.items} tasks</dd>
	</div>
</dl>

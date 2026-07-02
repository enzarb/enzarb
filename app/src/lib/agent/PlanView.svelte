<script lang="ts">
	import type { PlanEntryPayload } from './types';

	let { entries }: { entries: PlanEntryPayload[] } = $props();

	const MARK: Record<string, string> = {
		completed: '✓',
		in_progress: '◐',
		pending: '○'
	};
</script>

<div class="plan-view">
	<div class="plan-title">Plan</div>
	<ul>
		{#each entries as entry, i (i)}
			<li class={entry.status}>
				<span class="plan-mark">{MARK[entry.status] ?? '○'}</span>
				{entry.content}
			</li>
		{/each}
	</ul>
</div>

<style>
	.plan-view { border: 1px solid var(--color-border); border-radius: 6px; padding: 0.5rem 0.7rem; font-size: 12px; }
	.plan-title { color: var(--color-text-muted); font-size: 11px; margin-bottom: 0.3rem; text-transform: uppercase; letter-spacing: 0.03em; }
	ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.2rem; }
	li { display: flex; align-items: baseline; gap: 0.4rem; }
	li.completed { color: var(--color-text-muted); text-decoration: line-through; }
	.plan-mark { flex-shrink: 0; width: 1em; }
</style>

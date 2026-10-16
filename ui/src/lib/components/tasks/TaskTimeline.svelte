<script lang="ts">
	import type { TransitionSummary } from '#lib/tasks.ts';

	/** The transitions summary from `task_detail`, newest last. */
	let { transitions }: { transitions: TransitionSummary[] } = $props();

	const where = (s: { status: string | null; node: string | null }) =>
		[s.status, s.node].filter(Boolean).join(' · ') || '—';
</script>

{#if transitions.length}
	<ol class="ez-timeline">
		{#each transitions as t, i (i)}
			<li class="ez-timeline__item">
				<time class="ez-muted" datetime={t.at}>{new Date(t.at).toLocaleString()}</time>
				<span><code>{t.eventType ?? 'event'}</code></span>
				<span>{where(t.from)} → <strong>{where(t.to)}</strong></span>
				{#if t.effects}<span class="ez-muted">{t.effects} effect{t.effects === 1 ? '' : 's'}</span
					>{/if}
			</li>
		{/each}
	</ol>
{:else}
	<p class="ez-muted">No transitions yet.</p>
{/if}

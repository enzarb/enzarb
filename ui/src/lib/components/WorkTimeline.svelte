<script lang="ts">
	import type { TransitionView } from '#lib/work-items.ts';
	import { isTerminal, stateTone } from '#lib/work-items.ts';

	type Kind = 'done' | 'current' | 'failed';
	let { transitions, state }: { transitions: TransitionView[]; state: string } = $props();

	// The last transition is where the item is now; everything before it is history.
	const steps = $derived(
		transitions.map((t, i) => {
			const current = i === transitions.length - 1;
			const tone = stateTone(t.toState);
			const kind: Kind = current
				? tone === 'failed'
					? 'failed'
					: isTerminal(t.toState)
						? 'done'
						: 'current'
				: 'done';
			return { ...t, kind };
		})
	);
	const marks: Record<Kind, string> = { done: '✓', current: '●', failed: '✗' };
	const fmt = (iso: string) => new Date(iso).toISOString().replace('T', ' ').slice(0, 19) + ' UTC';
</script>

<ol class="checklist timeline" aria-label="Timeline" data-state={state}>
	{#each steps as s (s.id)}
		<li class="step {s.kind}" data-kind={s.kind}>
			<span class="mark" aria-hidden="true">{marks[s.kind]}</span>
			<span class="label">
				{#if s.fromState}<span class="muted">{s.fromState} →</span>{/if}
				{s.toState}
			</span>
			<time class="state muted" datetime={s.at}>{fmt(s.at)}</time>
			<span class="message muted">{s.eventType}</span>
		</li>
	{:else}
		<li class="step pending" data-kind="pending">
			<span class="mark" aria-hidden="true">○</span>
			<span class="label">No transitions recorded yet</span>
		</li>
	{/each}
</ol>

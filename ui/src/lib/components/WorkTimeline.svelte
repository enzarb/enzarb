<script lang="ts">
	import type { TransitionView } from '#lib/work-items.ts';
	import { isTerminal, stateTone } from '#lib/work-items.ts';
	import StepList, { type StepItem, type StepStatus } from './ui/StepList.svelte';

	type Kind = 'done' | 'current' | 'failed';
	/** A work item's history: one step per state transition, the latest one marked current. */
	let { transitions, state }: { transitions: TransitionView[]; state: string } = $props();

	const status: Record<Kind, StepStatus> = { done: 'done', current: 'active', failed: 'failed' };
	const fmt = (iso: string) => new Date(iso).toISOString().replace('T', ' ').slice(0, 19) + ' UTC';

	// The last transition is where the item is now; everything before it is history.
	const items = $derived<StepItem[]>(
		transitions.length === 0
			? [
					{
						id: 'none',
						status: 'pending',
						label: 'No transitions recorded yet',
						attrs: { 'data-kind': 'pending' }
					}
				]
			: transitions.map((t, i) => {
					const current = i === transitions.length - 1;
					const tone = stateTone(t.toState);
					const kind: Kind = current
						? tone === 'failed'
							? 'failed'
							: isTerminal(t.toState)
								? 'done'
								: 'current'
						: 'done';
					return {
						id: t.id,
						status: status[kind],
						prefix: t.fromState ? `${t.fromState} →` : undefined,
						label: t.toState,
						time: { iso: t.at, text: fmt(t.at) },
						message: t.eventType,
						attrs: { 'data-kind': kind }
					};
				})
	);
</script>

<StepList {items} label="Timeline" data-state={state} />

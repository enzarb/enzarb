<script lang="ts">
	import { decisionHeadline, type EventDecision } from '#lib/events.ts';
	import Badge from './ui/Badge.svelte';

	/** What tracks decided about an event: which workflow started, or why none did. */
	let { decision }: { decision: EventDecision } = $props();

	const head = $derived(decisionHeadline(decision));
	const tone = $derived(
		head.tone === 'ok' ? 'success' : head.tone === 'error' ? 'danger' : 'neutral'
	);
</script>

<div class="ez-decision" data-testid="event-decision">
	<Badge {tone}>{head.text}</Badge>
	{#if decision.errors.length > 0 || decision.reasons.length > 0}
		<ul class="ez-decision__why ez-muted">
			{#each decision.errors as e (e)}
				<li>{e}</li>
			{/each}
			{#each decision.reasons as r (r.workflow)}
				<li>
					<strong>{r.workflow}</strong>: {r.matched ? 'matched' : 'no match'}, {r.reason}
				</li>
			{/each}
		</ul>
	{/if}
</div>

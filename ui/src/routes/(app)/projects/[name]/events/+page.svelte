<script lang="ts">
	import { page } from '$app/state';
	import EventLog from '#lib/components/EventLog.svelte';
	import { watchEvents } from '#lib/live.ts';
	import { getEventPayload, listDecisions, listEvents } from '#lib/events.remote.ts';

	const name = $derived(page.params.name!);
	const events = $derived(listEvents({ name }));

	const ids = $derived((events.current ?? []).map((r) => r.id));
	const decided = $derived(ids.length > 0 ? listDecisions({ name, ids }) : undefined);
	const decisions = $derived(
		Object.fromEntries((decided?.current ?? []).map((d) => [d.eventId, d]))
	);

	// A new delivery arrives as a realtime frame; the list is refetched (a reconnect refetches too).
	$effect(() =>
		watchEvents(() => {
			void events.refresh();
			// Routing finishes a moment after the delivery is logged: look again shortly.
			void decided?.refresh();
			setTimeout(() => void decided?.refresh(), 3000);
		})
	);
</script>

<svelte:head><title>Events · {page.params.name} · enzarb</title></svelte:head>

<EventLog
	rows={events.current}
	{decisions}
	failed={!!events.error && !events.current}
	payload={async (id) => {
		const q = getEventPayload({ name, id });
		await q.refresh();
		return q.current ?? null;
	}}
	retry={() => void events.refresh()}
/>

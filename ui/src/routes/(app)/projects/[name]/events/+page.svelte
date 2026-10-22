<script lang="ts">
	import { page } from '$app/state';
	import EventLog from '#lib/components/EventLog.svelte';
	import { watchEvents } from '#lib/live.ts';
	import { getEventPayload, listEvents } from '#lib/events.remote.ts';

	const name = $derived(page.params.name!);
	const events = $derived(listEvents({ name }));

	// A new delivery arrives as a realtime frame; the list is refetched (a reconnect refetches too).
	$effect(() => watchEvents(() => void events.refresh()));
</script>

<svelte:head><title>Events · {page.params.name} · enzarb</title></svelte:head>

<EventLog
	rows={events.current}
	failed={!!events.error && !events.current}
	payload={async (id) => {
		const q = getEventPayload({ name, id });
		await q.refresh();
		return q.current ?? null;
	}}
	retry={() => void events.refresh()}
/>

<script lang="ts">
	import { untrack } from 'svelte';
	import Button from '#lib/components/form/Button.svelte';
	import TextField from '#lib/components/form/TextField.svelte';
	import Notice from '#lib/components/ui/Notice.svelte';
	import { appendEvents, isError, matches } from '#lib/session.ts';
	import type { SessionEvent } from '#lib/tasks.ts';
	import SessionEventView from './SessionEventView.svelte';

	/**
	 * A step's (or chat's) captured session as a conversation. `load(afterSeq)` returns the next
	 * page; while `live`, new events are tailed every `pollMs`. Search filters the loaded events.
	 */
	let {
		load,
		live = false,
		pollMs = 2000
	}: {
		load: (afterSeq: number) => Promise<SessionEvent[]>;
		live?: boolean;
		pollMs?: number;
	} = $props();

	let events = $state.raw<SessionEvent[]>([]);
	let loaded = $state(false);
	let failed = $state(false);
	let query = $state('');
	let busy = false;

	const shown = $derived(events.filter((e) => matches(e, query)));
	const firstError = $derived(events.find(isError));

	async function more() {
		if (busy) return;
		busy = true;
		try {
			for (let i = 0; i < 20; i++) {
				const after = events.length ? events[events.length - 1].seq : 0;
				const page = await load(after);
				events = appendEvents(events, page);
				if (page.length === 0 || page.length < 500) break;
			}
			failed = false;
		} catch {
			failed = true;
		} finally {
			busy = false;
			loaded = true;
		}
	}

	$effect(() => {
		void load;
		untrack(() => void more());
		if (!live) return;
		const t = setInterval(() => void more(), pollMs);
		return () => clearInterval(t);
	});

	function jumpToError() {
		if (!firstError) return;
		query = '';
		requestAnimationFrame(() =>
			document
				.getElementById(`seq-${firstError.seq}`)
				?.scrollIntoView({ behavior: 'smooth', block: 'center' })
		);
	}
</script>

<div class="ez-session" data-testid="session-inspector">
	<div class="ez-session__toolbar">
		<TextField label="Search this session" type="search" bind:value={query} />
		{#if firstError}
			<Button size="sm" variant="danger" onclick={jumpToError}>Jump to error</Button>
		{/if}
		{#if live}<span class="ez-badge ez-badge--info">live</span>{/if}
	</div>
	{#if failed}<Notice tone="error">Could not load the session.</Notice>{/if}
	{#if !loaded}
		<p class="ez-muted">Loading session…</p>
	{:else if events.length === 0}
		<p class="ez-muted">No session events captured.</p>
	{:else}
		<p class="ez-muted">
			{shown.length === events.length
				? `${events.length} events`
				: `${shown.length} of ${events.length} events`}
		</p>
		<div class="ez-session__events">
			{#each shown as e (e.seq)}
				<SessionEventView event={e} />
			{/each}
		</div>
	{/if}
</div>

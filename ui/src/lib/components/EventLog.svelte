<script lang="ts">
	import type { EventDecision as Decision, EventRow } from '#lib/events.ts';
	import EventDecision from './EventDecision.svelte';
	import Button from './form/Button.svelte';
	import EmptyState from './ui/EmptyState.svelte';
	import Notice from './ui/Notice.svelte';
	import Section from './ui/Section.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/**
	 * Webhook deliveries logged for a project's repositories, newest first. Each row shows what
	 * enzarb-tracks decided about it (once it has), and expands to the raw payload.
	 */
	let {
		rows,
		failed = false,
		payload,
		retry,
		decisions = {},
		runLinks = {}
	}: {
		/** Runs each event started (by event id), linked to their detail page. */
		runLinks?: Record<number, { id: string; workflow: string; href: string }[] | undefined>;
		/** Routing decisions by event id; an event without one shows none yet. */
		decisions?: Record<number, Decision | undefined>;
		/** `undefined` while the first load is in flight. */
		rows: EventRow[] | undefined;
		failed?: boolean;
		payload: (id: number) => Promise<unknown>;
		retry: () => void;
	} = $props();

	let open = $state<Record<number, { json?: string; error?: boolean } | undefined>>({});

	async function toggle(id: number, isOpen: boolean) {
		if (!isOpen) return;
		if (open[id]?.json) return;
		open[id] = {};
		try {
			const p = await payload(id);
			open[id] = { json: JSON.stringify(p, null, 2) };
		} catch {
			open[id] = { error: true };
		}
	}

	const label = (r: EventRow) => (r.action ? `${r.event} ${r.action}` : r.event);
</script>

{#if failed}
	<Notice tone="error" title="Could not load events">
		Something went wrong while loading the events.
		{#snippet actions()}<Button onclick={retry}>Retry</Button>{/snippet}
	</Notice>
{:else if rows === undefined}
	<Skeleton label="Loading events" rows={3} />
{:else if rows.length === 0}
	<EmptyState
		title="No events yet"
		description="GitHub deliveries for this project's repositories are logged here, with what was decided about each."
	/>
{:else}
	<Section title="Events" description="Each delivery and the workflow it started, if any.">
		<ul class="ez-event-log" data-testid="event-log">
			{#each rows as r (r.id)}
				<li>
					<details ontoggle={(e) => toggle(r.id, e.currentTarget.open)}>
						<summary>
							<strong>{label(r)}</strong>
							<span class="ez-muted">{r.repo}</span>
							{#if r.summary}<span>{r.summary}</span>{/if}
							<span class="ez-muted">
								{#if r.sender}{r.sender} ·{/if}
								<time datetime={r.receivedAt}>{new Date(r.receivedAt).toLocaleString()}</time>
							</span>
						</summary>
						{#if open[r.id]?.json}
							<pre>{open[r.id]!.json}</pre>
						{:else if open[r.id]?.error}
							<p class="ez-muted">Could not load the payload.</p>
						{:else}
							<p class="ez-muted">Loading…</p>
						{/if}
					</details>
					{#if decisions[r.id]}
						<EventDecision decision={decisions[r.id]!} />
					{/if}
					{#each runLinks[r.id] ?? [] as run (run.id)}
						<a class="ez-event-log__run" href={run.href}>View run of {run.workflow}</a>
					{/each}
				</li>
			{/each}
		</ul>
	</Section>
{/if}

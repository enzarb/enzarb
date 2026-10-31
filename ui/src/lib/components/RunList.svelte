<script lang="ts">
	import { repoContext } from '#lib/github-links.ts';
	import { runTone, type RunSummary } from '#lib/runs.ts';
	import SubjectLink from './SubjectLink.svelte';
	import Button from './form/Button.svelte';
	import Badge from './ui/Badge.svelte';
	import EmptyState from './ui/EmptyState.svelte';
	import Notice from './ui/Notice.svelte';
	import Section from './ui/Section.svelte';
	import Skeleton from './ui/Skeleton.svelte';

	/** The Project's workflow runs, newest first; each links to its detail. */
	let {
		runs,
		base,
		failed = false,
		retry
	}: {
		/** `undefined` while the first load is in flight. */
		runs: RunSummary[] | undefined;
		/** `/projects/<name>/runs` */
		base: string;
		failed?: boolean;
		retry: () => void;
	} = $props();
	const ctx = $derived(repoContext((runs ?? []).map((r) => r.repo)));
</script>

{#if failed}
	<Notice tone="error" title="Could not load runs">
		Something went wrong while loading the runs.
		{#snippet actions()}<Button onclick={retry}>Retry</Button>{/snippet}
	</Notice>
{:else if runs === undefined}
	<Skeleton label="Loading runs" rows={3} />
{:else if runs.length === 0}
	<EmptyState
		title="No runs yet"
		description="When an event matches a workflow, its run appears here, node by node."
	/>
{:else}
	<Section title="Runs" description="Workflow runs in this project's sandbox.">
		<ul class="ez-runs" data-testid="run-list">
			{#each runs as r (r.id)}
				{@const t = runTone(r.status)}
				<li class="ez-runs__row">
					<a href="{base}/{r.id}" class="ez-runs__link">
						<strong>{r.workflow}</strong>
						<Badge tone={t.tone} hollow={t.hollow}>{r.status}</Badge>
						{#if r.test}<Badge tone="info" plain>Test</Badge>{/if}
					</a>
					{#if r.subject}
						<SubjectLink
							repo={r.repo}
							number={r.subject.number}
							url={r.subject.url}
							context={ctx}
						/>
					{:else}
						<span class="ez-muted">{r.repo}</span>
					{/if}
					{#if r.reason}<span class="ez-muted">{r.reason}</span>{/if}
					<time class="ez-muted" datetime={r.createdAt}
						>{new Date(r.createdAt).toLocaleString()}</time
					>
				</li>
			{/each}
		</ul>
	</Section>
{/if}

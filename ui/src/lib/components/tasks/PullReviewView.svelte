<script lang="ts">
	import {
		checksText,
		summarizeChecks,
		type MergeStep,
		type PullReview
	} from '#lib/pull-review.ts';
	import RunLink from '../RunLink.svelte';
	import Notice from '../ui/Notice.svelte';
	import Section from '../ui/Section.svelte';
	import Skeleton from '../ui/Skeleton.svelte';
	import DiffFile from './DiffFile.svelte';
	import MergeSteps from './MergeSteps.svelte';

	/** A pull request in review: enzarb's notes, CI summary, the inline diff and the merge steps. */
	let {
		review,
		project,
		url,
		steps = [],
		merging = false
	}: {
		/** `undefined` while loading, `null` when it could not be read. */
		review: PullReview | null | undefined;
		project: string;
		/** The pull request on GitHub. */
		url: string;
		steps?: MergeStep[];
		merging?: boolean;
	} = $props();

	const summary = $derived(review ? summarizeChecks(review.checks) : null);
</script>

{#if steps.length || merging}
	<Section title="Approve & merge">
		<MergeSteps {steps} running={merging} />
	</Section>
{/if}

{#if review === undefined}
	<Skeleton label="Loading the pull request" rows={3} />
{:else if review === null}
	<Notice tone="warning" title="The pull request could not be read">
		GitHub or enzarb-tracks may be unreachable.
	</Notice>
{:else}
	{#each review.problems as p (p)}<Notice tone="warning">{p}</Notice>{/each}
	{#if review.notes}
		<Section title="Review notes">
			{#snippet actions()}
				{#if review.notes?.run}
					<RunLink
						workflow={review.notes.run.workflow}
						id={review.notes.run.id}
						href="/projects/{encodeURIComponent(project)}/runs/{review.notes.run.id}"
					/>
				{/if}
			{/snippet}
			<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized by renderMarkdown on the server -->
			<div class="ez-md" data-testid="review-notes">{@html review.notes.html}</div>
		</Section>
	{/if}
	{#if summary}
		<p data-testid="checks-summary" class:ez-diff__bad={summary.failed.length > 0}>
			<strong>Checks:</strong>
			{checksText(summary)}
		</p>
	{/if}
	<Section
		title="Changes"
		description="{review.files.length} file{review.files.length === 1 ? '' : 's'} changed"
	>
		{#each review.files as f (f.filename)}
			<DiffFile file={f} {url} open={review.files.length <= 5} />
		{:else}
			<p class="ez-muted">No changed files were returned.</p>
		{/each}
		{#if review.filesTruncated}
			<p class="ez-muted">
				Only the first files are shown.
				<a href="{url}/files" target="_blank" rel="noopener noreferrer">See all on GitHub</a>
			</p>
		{/if}
	</Section>
{/if}

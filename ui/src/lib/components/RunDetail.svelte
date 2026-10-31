<script lang="ts">
	import { repoContext, runLabel } from '#lib/github-links.ts';
	import { isEnded, runTone, type RunDetail } from '#lib/runs.ts';
	import Button from './form/Button.svelte';
	import SubjectLink from './SubjectLink.svelte';
	import RunNodes from './RunNodes.svelte';
	import ConfirmDialog from './ui/ConfirmDialog.svelte';
	import KeyValueList from './ui/KeyValueList.svelte';
	import Section from './ui/Section.svelte';

	/** One run: its facts, a Stop button while it is live, and each node visit with what ran. */
	let {
		run,
		stop
	}: {
		run: RunDetail;
		/** Asks the executor to stop the run; the page refreshes itself from the server. */
		stop: () => Promise<void>;
	} = $props();

	let confirming = $state(false);
	const t = $derived(runTone(run.status));
	const live = $derived(!isEnded(run));
</script>

<Section title={runLabel(run.workflow, run.id)}>
	{#snippet actions()}
		{#if live}
			<Button variant="danger" disabled={run.stopRequested} onclick={() => (confirming = true)}
				>{run.stopRequested ? 'Stopping…' : 'Stop'}</Button
			>
		{/if}
	{/snippet}
	{#if run.subject}
		<p>
			<SubjectLink
				repo={run.repo}
				number={run.subject.number}
				url={run.subject.url}
				context={repoContext([run.repo])}
			/>
		</p>
	{:else}
		<p class="ez-muted">{run.repo}</p>
	{/if}
	<KeyValueList
		items={[
			{ term: 'Status', value: run.status, tone: t.tone },
			...(run.reason ? [{ term: 'Reason', value: run.reason }] : []),
			{ term: 'Commit', value: run.sha.slice(0, 7) },
			{ term: 'Started', value: new Date(run.createdAt).toLocaleString() },
			...(run.test ? [{ term: 'Kind', value: 'Test run', tone: 'info' as const }] : [])
		]}
	/>
</Section>

<RunNodes {run} />

<ConfirmDialog
	bind:open={confirming}
	title="Stop this run?"
	description="The running step is interrupted and the run ends cancelled."
	confirmLabel="Stop run"
	tone="danger"
	onconfirm={stop}
/>

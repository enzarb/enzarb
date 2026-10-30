<script lang="ts">
	import { branchUrl, isEnded, nodeText, runTone, usageText, type RunDetail } from '#lib/runs.ts';
	import Button from './form/Button.svelte';
	import Badge from './ui/Badge.svelte';
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

<Section title="{run.workflow} · {run.repo}{run.subject ? `#${run.subject.number}` : ''}">
	{#snippet actions()}
		{#if live}
			<Button variant="danger" disabled={run.stopRequested} onclick={() => (confirming = true)}
				>{run.stopRequested ? 'Stopping…' : 'Stop'}</Button
			>
		{/if}
	{/snippet}
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

<Section title="Nodes" description="Each visit of a node, as it ran in the sandbox.">
	{#if run.nodeRuns.length === 0}
		<p class="ez-muted">No node has run yet.</p>
	{:else}
		<ol class="ez-noderuns" data-testid="node-runs">
			{#each run.nodeRuns as n (n.node + ':' + n.visit)}
				{@const nt = runTone(n.status)}
				<li aria-label="Node {n.node}">
					<div class="ez-noderuns__head">
						<strong>{n.node}</strong>
						{#if n.visit > 1}<span class="ez-muted">visit {n.visit}</span>{/if}
						<Badge tone={nt.tone} hollow={nt.hollow}>{n.status}</Badge>
						{#if n.mode}<span class="ez-muted">mode <code>{n.mode}</code></span>{/if}
						{#if usageText(n.usage)}<span class="ez-muted">{usageText(n.usage)}</span>{/if}
					</div>
					{#if nodeText(n.rendered)}
						<pre class="ez-noderuns__text">{nodeText(n.rendered)}</pre>
					{/if}
					{#if n.outputs}
						{#if n.outputs.result !== undefined && n.outputs.result !== null}
							<details>
								<summary>Result</summary>
								<pre>{JSON.stringify(n.outputs.result, null, 2)}</pre>
							</details>
						{/if}
						{#if n.outputs.exit_code !== undefined}
							<p class="ez-muted">exit code {n.outputs.exit_code}</p>
						{/if}
					{/if}
					{#each n.git ?? [] as g (g.branch)}
						<p class="ez-muted">
							{#if branchUrl(g)}
								Pushed
								<a href={branchUrl(g)} rel="noreferrer noopener" target="_blank"
									><code>{g.branch}</code></a
								>
							{:else}
								Branch <code>{g.branch}</code>, not pushed
							{/if}
							{#if g.head_sha}at <code>{g.head_sha.slice(0, 7)}</code>{/if}
						</p>
					{/each}
					{#if n.error}<p class="ez-noderuns__error" role="alert">{n.error}</p>{/if}
				</li>
			{/each}
		</ol>
	{/if}
</Section>

<ConfirmDialog
	bind:open={confirming}
	title="Stop this run?"
	description="The running step is interrupted and the run ends cancelled."
	confirmLabel="Stop run"
	tone="danger"
	onconfirm={stop}
/>

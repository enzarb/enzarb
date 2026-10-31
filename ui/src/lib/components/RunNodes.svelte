<script lang="ts">
	import {
		actionText,
		actionUrl,
		branchUrl,
		nodeText,
		runTone,
		usageText,
		waitText,
		type RunDetail
	} from '#lib/runs.ts';
	import Badge from './ui/Badge.svelte';
	import Section from './ui/Section.svelte';

	/** The node visits of one run, as they ran in the sandbox. Shared by the Runs and Tasks pages. */
	let { run }: { run: Pick<RunDetail, 'nodeRuns'> } = $props();
</script>

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
					{#if actionText(n)}
						<p data-testid="node-action">
							{#if actionUrl(n)}
								<a href={actionUrl(n)} rel="noreferrer noopener" target="_blank">{actionText(n)}</a>
							{:else}
								{actionText(n)}
							{/if}
						</p>
						{#if n.rendered?.body}<pre class="ez-noderuns__text">{n.rendered.body}</pre>{/if}
					{/if}
					{#if n.rendered?.type === 'wait'}
						<p data-testid="node-wait" class="ez-muted">
							{#if n.status === 'waiting'}
								Waiting for {waitText(n)}{n.wait?.until
									? `, until ${new Date(n.wait.until).toLocaleString()}`
									: ', no timeout'}
							{:else if n.wait?.timed_out}
								Timed out waiting for {waitText(n)}
							{:else}
								Resumed by {waitText(n)}{n.wait?.resumed_by_event
									? ` (event ${n.wait.resumed_by_event})`
									: ''}
							{/if}
						</p>
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

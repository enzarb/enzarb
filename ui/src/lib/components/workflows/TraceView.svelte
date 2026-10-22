<script lang="ts">
	import Button from '../form/Button.svelte';
	import TextArea from '../form/TextArea.svelte';
	import Badge from '../ui/Badge.svelte';
	import {
		assumeTemplate,
		type NodeDoc,
		type Trace,
		type TraceStep
	} from '#lib/workflows-types.ts';

	/**
	 * One workflow's dry-run walk for one event: every node it entered with its rendered templates,
	 * the edge it took, and how it stopped. Where it needs an outcome (an agent's result, a
	 * command's exit code...) the user assumes one and the walk continues.
	 */
	let {
		trace,
		nodes,
		busy,
		assume,
		show
	}: {
		trace: Trace;
		/** The workflow's nodes, for result schemas in assume templates. */
		nodes: readonly NodeDoc[];
		busy: boolean;
		assume: (node: string, outputs: unknown) => void;
		show: () => void;
	} = $props();

	const end = $derived(trace.end);
	let outputsText = $state('');
	let parseError = $state('');
	$effect.pre(() => {
		if (end.kind === 'needs_outcome') {
			const n = nodes.find((x) => x.id === end.node);
			outputsText = JSON.stringify(assumeTemplate(n, end.node_type), null, 2);
			parseError = '';
		}
	});

	function submit(node: string) {
		try {
			assume(node, JSON.parse(outputsText) as unknown);
		} catch {
			parseError = 'Not valid JSON.';
		}
	}

	const endTone = { done: 'success', failed: 'danger', cancelled: 'neutral' } as const;

	function details(s: TraceStep): [string, string][] {
		const r = s.rendered;
		if (!r) return [];
		const out: [string, string][] = [];
		const add = (k: string, v: unknown) => {
			if (v === undefined || v === null || v === '' || (Array.isArray(v) && !v.length)) return;
			out.push([k, typeof v === 'string' ? v : JSON.stringify(v, null, 2)]);
		};
		for (const [k, v] of Object.entries(r)) if (k !== 'type') add(k, v);
		if (s.mode) add('tools', s.mode.tools ?? 'every built-in tool');
		if (s.assumed !== undefined) add('assumed outputs', s.assumed);
		return out;
	}
</script>

<div class="ez-wf-trace">
	<div class="ez-cluster">
		<span class="ez-mono">{trace.workflow}</span>
		{#if trace.forced}<Badge tone="info">traced on request</Badge>{/if}
		<Button size="sm" onclick={show}>Show on the graph</Button>
	</div>
	<ol class="ez-wf-trace__steps">
		{#each trace.steps as s, i (i)}
			<li>
				<details>
					<summary>
						<span class="ez-mono">{s.node}</span>
						<span class="ez-muted"
							>{s.node_type}{s.visit > 1 ? `, visit ${s.visit}` : ''}{s.mode
								? `, mode ${s.mode.id}`
								: ''}</span
						>
						{#if s.transition?.kind === 'moved'}
							<span
								>→ <span class="ez-mono">{s.transition.to}</span> (edge {s.transition.edge +
									1})</span
							>
						{:else if s.transition?.kind === 'ended'}
							<span
								>ends {s.transition.status}{s.transition.reason
									? `: ${s.transition.reason}`
									: ''}</span
							>
						{/if}
					</summary>
					<dl class="ez-wf-trace__details">
						{#each details(s) as [k, v] (k)}
							<dt>{k}</dt>
							<dd><pre class="ez-mono">{v}</pre></dd>
						{/each}
					</dl>
				</details>
			</li>
		{/each}
	</ol>
	{#if end.kind === 'ended'}
		<p>
			<Badge tone={endTone[end.status]}>{end.status}</Badge>
			{end.node ? `at ${end.node}` : ''}{end.reason ? `: ${end.reason}` : ''}
		</p>
	{:else if end.kind === 'step_limit'}
		<p>
			<Badge tone="warning">stopped</Badge> after {end.steps} steps (a loop without a way out?).
		</p>
	{:else}
		<div class="ez-stack ez-wf-trace__assume">
			<p>
				<Badge tone="info">needs an outcome</Badge>
				The walk stops at <span class="ez-mono">{end.node}</span> ({end.node_type}): a dry run does
				not run it. Assume its outputs to see which edge it takes.
			</p>
			{#if end.edges.length}
				<ul class="ez-wf-trace__edges" aria-label="Edges of {end.node}">
					{#each end.edges as e, i (i)}
						<li>
							<span class="ez-mono">{e.if ?? 'otherwise'}</span> →
							<span class="ez-mono">{e.to}</span>
						</li>
					{/each}
				</ul>
			{/if}
			<TextArea
				label="Assumed outputs of {end.node} ({end.outputs.join(', ')})"
				rows={6}
				class="ez-control ez-mono"
				spellcheck={false}
				bind:value={outputsText}
				issues={parseError ? [parseError] : []}
			/>
			<div>
				<Button size="sm" variant="primary" disabled={busy} onclick={() => submit(end.node)}
					>Assume and continue</Button
				>
			</div>
		</div>
	{/if}
</div>

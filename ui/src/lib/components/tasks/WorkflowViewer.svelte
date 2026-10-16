<script lang="ts">
	import Notice from '#lib/components/ui/Notice.svelte';
	import Section from '#lib/components/ui/Section.svelte';
	import { originLabel, originUrl, type Origin, type WorkflowViewDoc } from '#lib/tasks.ts';
	import PhaseGraph from './PhaseGraph.svelte';

	/** Read-only view of `workflow_v`: bindings, the phases of each workflow, and lint. */
	let { view }: { view: WorkflowViewDoc } = $props();

	let openGraph = $state<string | null>(null);
	const fmt = (x: unknown) => (typeof x === 'string' ? x : JSON.stringify(x));
</script>

{#snippet origin(o: Origin | undefined)}
	{@const url = originUrl(o)}
	{#if url}
		<a class="ez-badge ez-badge--info" href={url} target="_blank" rel="noopener noreferrer"
			>{originLabel(o)}</a
		>
	{:else}
		<span class="ez-badge">{originLabel(o)}</span>
	{/if}
{/snippet}

<div class="ez-stack" data-testid="workflow-viewer">
	{#if view.lint?.length}
		<Notice tone="warning" title="Lint">
			<ul>
				{#each view.lint as l, i (i)}
					<li>
						<strong>{l.level}</strong>
						{l.message}{#if l.path}
							<code>{l.path}</code>{/if}
					</li>
				{/each}
			</ul>
		</Notice>
	{/if}

	<Section title="Bindings">
		{#each view.bindings as b (b.id)}
			<div class="ez-wf-binding">
				<code>{b.id}</code>
				<span>on <code>{fmt(b.on ?? '—')}</code></span>
				{#if b.if}<span>if <code>{b.if}</code></span>{/if}
				<span>→ <code>{b.use}</code></span>
				{#if b.disabled}<span class="ez-badge">disabled</span>{/if}
				{@render origin(b.origin)}
			</div>
		{:else}
			<p class="ez-muted">No bindings.</p>
		{/each}
	</Section>

	{#each view.workflows as w (w.ref)}
		<Section title={w.ref}>
			{#snippet actions()}{@render origin(w.origin)}{/snippet}
			<ol class="ez-wf-phases">
				{#each w.phases as p (p.id)}
					{@const key = `${w.ref}#${p.id}`}
					<li class="ez-wf-phase">
						<div class="ez-cluster">
							<strong>{p.id}</strong>
							{#if p.use}<code>{p.use}</code>{/if}
							{@render origin(p.origin)}
						</div>
						{#if p.params && Object.keys(p.params).length}
							<p class="ez-muted">
								params: {#each Object.entries(p.params) as [k, val] (k)}<code>{k}={fmt(val)}</code>
								{/each}
							</p>
						{/if}
						{#if p.with && Object.keys(p.with).length}
							<p class="ez-muted">
								with: {#each Object.entries(p.with) as [k, val] (k)}<code>{k}={fmt(val)}</code>
								{/each}
							</p>
						{/if}
						{#if p.exits && Object.keys(p.exits).length}
							<p class="ez-muted">
								exits: {#each Object.entries(p.exits) as [k, val] (k)}<code>{k} → {fmt(val)}</code>
								{/each}
							</p>
						{/if}
						{#if p.graph?.nodes.length}
							<button
								type="button"
								class="ez-link-button"
								aria-expanded={openGraph === key}
								onclick={() => (openGraph = openGraph === key ? null : key)}
								>{openGraph === key ? 'Hide internals' : 'Show internals'}</button
							>
							{#if openGraph === key}<PhaseGraph graph={p.graph} />{/if}
						{/if}
					</li>
				{/each}
			</ol>
		</Section>
	{/each}
</div>

<script lang="ts" module>
	import type { WorkflowNode } from '#lib/workflow/definition.ts';

	export type FlowNodeData = {
		node: WorkflowNode;
		start: boolean;
		problems: string[];
	};
</script>

<script lang="ts">
	import { Handle, Position } from '@xyflow/svelte';

	/** One workflow node on the canvas, styled by type; problems show as a count and a tooltip. */
	let { data, selected = false }: { data: FlowNodeData; selected?: boolean } = $props();

	const n = $derived(data.node);
	const detail = $derived.by(() => {
		switch (n.type) {
			case 'command':
				return n.run ?? '';
			case 'agent':
				return typeof n.prompt === 'object' ? n.prompt.file : (n.prompt ?? '').split('\n')[0];
			case 'wait':
				return (n.events ?? []).join(', ');
			case 'action':
				return n.action ?? '';
			case 'end':
				return n.status ?? '';
		}
		return '';
	});
</script>

<div
	class={['wf-node', `wf-node--${n.type}`, { 'wf-node--selected': selected }]}
	data-testid="wf-node-{n.id}"
	title={data.problems.join('\n') || undefined}
>
	<Handle type="target" position={Position.Top} />
	<div class="wf-node__head">
		<span class="wf-node__type">{n.type}</span>
		{#if data.start}<span class="wf-node__start">start</span>{/if}
		{#if data.problems.length}
			<span class="wf-node__problems" aria-label="{data.problems.length} problems"
				>{data.problems.length}</span
			>
		{/if}
	</div>
	<div class="wf-node__id">{n.id}</div>
	{#if detail}<div class="wf-node__detail">{detail}</div>{/if}
	{#if n.type !== 'end'}<Handle type="source" position={Position.Bottom} />{/if}
</div>

<style>
	.wf-node {
		width: 200px;
		min-height: 64px;
		box-sizing: border-box;
		padding: var(--ez-space-1) var(--ez-space-2);
		border: 1px solid var(--ez-border-strong);
		border-left: 4px solid var(--wf-color);
		border-radius: var(--ez-radius-sm);
		background: var(--ez-surface);
		color: var(--ez-text);
		font-size: 0.8rem;
		--wf-color: var(--ez-text-subtle);
	}
	.wf-node--command {
		--wf-color: var(--ez-info-fg);
	}
	.wf-node--agent {
		--wf-color: var(--ez-accent);
	}
	.wf-node--wait {
		--wf-color: var(--ez-warning-fg);
	}
	.wf-node--action {
		--wf-color: var(--ez-success-fg);
	}
	.wf-node--end {
		--wf-color: var(--ez-text-muted);
		border-radius: 999px;
	}
	.wf-node--selected {
		outline: 2px solid var(--ez-accent);
	}
	.wf-node__head {
		display: flex;
		gap: var(--ez-space-1);
		align-items: center;
		color: var(--ez-text-subtle);
		text-transform: uppercase;
		font-size: 0.65rem;
		letter-spacing: 0.04em;
	}
	.wf-node__start {
		color: var(--ez-accent-text);
	}
	.wf-node__problems {
		margin-left: auto;
		padding: 0 6px;
		border-radius: 999px;
		background: var(--ez-danger-solid);
		color: var(--ez-danger-solid-fg);
	}
	.wf-node__id {
		font-weight: 600;
		font-family: var(--ez-font-mono, monospace);
	}
	.wf-node__detail {
		color: var(--ez-text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
</style>

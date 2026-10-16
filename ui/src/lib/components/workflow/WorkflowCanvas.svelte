<script lang="ts">
	import '@xyflow/svelte/dist/style.css';
	import {
		Background,
		Controls,
		MarkerType,
		SvelteFlow,
		type Connection,
		type Edge,
		type Node
	} from '@xyflow/svelte';
	import type { Definition } from '#lib/workflow/definition.ts';
	import { drawnEdges, layout } from '#lib/workflow/edit.ts';
	import FlowNode, { type FlowNodeData } from './FlowNode.svelte';

	/**
	 * The workflow as a graph: auto-laid-out (dagre), nodes styled per type, edges labelled with
	 * their condition and order. Click a node to select it; drag from a node's bottom handle to
	 * another node to add an edge.
	 */
	let {
		def,
		problems,
		selected,
		onselect,
		onconnect
	}: {
		def: Definition;
		problems: Map<string, string[]>;
		selected: string | null;
		onselect: (id: string | null) => void;
		onconnect: (from: string, to: string) => void;
	} = $props();

	const nodeTypes = { workflow: FlowNode };

	let nodes = $derived.by<Node<FlowNodeData>[]>(() => {
		const pos = layout(def);
		return def.nodes.map((n) => ({
			id: n.id,
			type: 'workflow',
			position: pos.get(n.id) ?? { x: 0, y: 0 },
			selected: n.id === selected,
			data: { node: n, start: n.id === def.start, problems: problems.get(n.id) ?? [] }
		}));
	});

	let edges = $derived.by<Edge[]>(() =>
		drawnEdges(def).map((e) => ({
			id: e.id,
			source: e.source,
			target: e.target,
			label: e.label,
			class: `wf-edge wf-edge--${e.kind}`,
			markerEnd: { type: MarkerType.ArrowClosed },
			animated: e.kind === 'limit'
		}))
	);
</script>

<div class="wf-canvas" data-testid="workflow-canvas">
	<SvelteFlow
		bind:nodes
		bind:edges
		{nodeTypes}
		fitView
		nodesDraggable={false}
		deleteKey={null}
		onnodeclick={({ node }) => onselect(node.id)}
		onpaneclick={() => onselect(null)}
		onconnect={(c: Connection) => onconnect(c.source, c.target)}
	>
		<Background />
		<Controls showLock={false} />
	</SvelteFlow>
</div>

<style>
	.wf-canvas {
		height: 100%;
		min-height: 420px;
		border: 1px solid var(--ez-border);
		border-radius: var(--ez-radius-sm);
		background: var(--ez-surface-sunken);
	}
	.wf-canvas :global(.wf-edge--else .svelte-flow__edge-path) {
		stroke-dasharray: 6 4;
	}
	.wf-canvas :global(.wf-edge--limit .svelte-flow__edge-path) {
		stroke: var(--ez-warning-fg);
	}
</style>

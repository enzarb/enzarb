<script lang="ts">
	import '@xyflow/svelte/dist/style.css';
	import dagre from '@dagrejs/dagre';
	import {
		Background,
		Controls,
		MarkerType,
		SvelteFlow,
		type Edge,
		type Node
	} from '@xyflow/svelte';
	import type { GraphEdge, GraphNode } from '#lib/tasks.ts';

	/** A phase's internals as a read-only, auto-laid-out graph (dagre). */
	let { graph }: { graph: { nodes: GraphNode[]; edges: GraphEdge[] } } = $props();

	const W = 170;
	const H = 44;

	const laidOut = $derived.by(() => {
		const g = new dagre.graphlib.Graph({ multigraph: true });
		g.setGraph({ rankdir: 'TB', nodesep: 40, ranksep: 60 });
		g.setDefaultEdgeLabel(() => ({}));
		for (const n of graph.nodes) g.setNode(n.id, { width: W, height: H });
		graph.edges.forEach((e, i) => {
			if (g.hasNode(e.from) && g.hasNode(e.to)) g.setEdge(e.from, e.to, {}, String(i));
		});
		dagre.layout(g);
		const nodes: Node[] = graph.nodes.map((n) => {
			const p = g.node(n.id);
			return {
				id: n.id,
				position: { x: (p?.x ?? 0) - W / 2, y: (p?.y ?? 0) - H / 2 },
				data: { label: `${n.label ?? n.id} (${n.kind})` },
				class: `ez-phase-node ez-phase-node--${n.kind}`,
				draggable: false,
				connectable: false
			};
		});
		const edges: Edge[] = graph.edges.map((e, i) => ({
			id: `e${i}`,
			source: e.from,
			target: e.to,
			label: e.label,
			markerEnd: { type: MarkerType.ArrowClosed }
		}));
		return { nodes, edges };
	});

	let nodes = $derived(laidOut.nodes);
	let edges = $derived(laidOut.edges);
</script>

<div class="ez-phase-graph" data-testid="phase-graph">
	<SvelteFlow
		bind:nodes
		bind:edges
		fitView
		nodesDraggable={false}
		nodesConnectable={false}
		elementsSelectable={false}
		deleteKey={null}
	>
		<Background />
		<Controls showLock={false} />
	</SvelteFlow>
</div>

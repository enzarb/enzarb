<script lang="ts">
	import {
		layoutWorkflow,
		NODE_H,
		NODE_W,
		type LaidEdge,
		type LaidNode,
		type WorkflowDoc
	} from '#lib/workflows-types.ts';

	/**
	 * One workflow as a top-down graph. The layout is computed from the edges (positions are not
	 * stored); loops are drawn around the right side. Nodes are buttons that select them.
	 */
	let {
		workflow,
		selected,
		errors = [],
		visited = [],
		taken = [],
		select
	}: {
		workflow: WorkflowDoc;
		selected: string | null;
		/** Node ids with errors. */
		errors?: readonly string[];
		/** Node ids a dry-run trace visited, highlighted. */
		visited?: readonly string[];
		/** Edges the trace took, as `from:index`. */
		taken?: readonly string[];
		select: (id: string) => void;
	} = $props();

	const uid = $props.id();
	const layout = $derived(layoutWorkflow(workflow));
	const at = $derived(new Map(layout.nodes.map((n) => [n.id, n])));
	const nodeOf = $derived(new Map(workflow.nodes.map((n) => [n.id, n])));

	function subtitle(id: string): string {
		const n = nodeOf.get(id);
		if (!n) return '';
		switch (n.type) {
			case 'agent':
				return `agent · ${n.mode || '?'}`;
			case 'command':
				return n.task ? `mise run ${n.task}` : 'command · shell';
			case 'action':
				return n.action.replaceAll('_', ' ');
			case 'wait':
				return `wait · ${n.on.join(', ') || '?'}`;
			case 'end':
				return `end · ${n.status}`;
		}
	}

	const clip = (s: string, max: number) => (s.length > max ? `${s.slice(0, max - 1)}…` : s);

	function path(e: LaidEdge, from: LaidNode, to: LaidNode): { d: string; lx: number; ly: number } {
		if (!e.back) {
			const x1 = from.x + NODE_W / 2;
			const y1 = from.y + NODE_H;
			const x2 = to.x + NODE_W / 2;
			const y2 = to.y;
			const my = (y1 + y2) / 2;
			return {
				d: `M${x1},${y1} C${x1},${my} ${x2},${my} ${x2},${y2 - 2}`,
				lx: (x1 + x2) / 2,
				ly: my
			};
		}
		// Loops and same-layer edges: out of the right side, around, into the target's right side.
		const bulge = 36 + 10 * e.index;
		const x1 = from.x + NODE_W;
		const y1 = from.y + NODE_H / 2 + 6;
		const x2 = to.x + NODE_W;
		const y2 = to.y + NODE_H / 2 - 6;
		const cx = Math.max(x1, x2) + bulge;
		return {
			d: `M${x1},${y1} C${cx},${y1} ${cx},${y2} ${x2 + 2},${y2}`,
			lx: cx - bulge / 4,
			ly: (y1 + y2) / 2
		};
	}

	function onkey(e: KeyboardEvent, id: string) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			select(id);
		}
	}
</script>

<div class="ez-wf-graph">
	<svg
		width={layout.width}
		height={layout.height}
		viewBox="0 0 {layout.width} {layout.height}"
		role="group"
		aria-label="Graph of {workflow.id}"
	>
		<defs>
			<marker
				id="{uid}-arrow"
				viewBox="0 0 10 10"
				refX="9"
				refY="5"
				markerWidth="7"
				markerHeight="7"
				orient="auto-start-reverse"
			>
				<path d="M0,0 L10,5 L0,10 z" class="ez-wf-graph__arrow" />
			</marker>
		</defs>
		{#each layout.edges as e (`${e.from}:${e.index}`)}
			{@const from = at.get(e.from)}
			{@const to = at.get(e.to)}
			{#if from && to}
				{@const p = path(e, from, to)}
				<g
					class="ez-wf-edge"
					class:ez-wf-edge--taken={taken.includes(`${e.from}:${e.index}`)}
					data-edge="{e.from}->{e.to}"
				>
					<path d={p.d} marker-end="url(#{uid}-arrow)" />
					{#if e.label}
						<text x={p.lx} y={p.ly} text-anchor="middle" dominant-baseline="middle">
							<title>{e.label}</title>{clip(e.label, 26)}
						</text>
					{/if}
				</g>
			{/if}
		{/each}
		{#each layout.nodes as n (n.id)}
			{@const type = nodeOf.get(n.id)?.type ?? 'agent'}
			<g
				class="ez-wf-node ez-wf-node--{type}"
				class:ez-wf-node--selected={n.id === selected}
				class:ez-wf-node--error={errors.includes(n.id)}
				class:ez-wf-node--visited={visited.includes(n.id)}
				class:ez-wf-node--orphan={n.orphan}
				transform="translate({n.x},{n.y})"
				role="button"
				tabindex="0"
				aria-pressed={n.id === selected}
				aria-label="{n.id}, {subtitle(n.id)}{n.id === workflow.start
					? ', start'
					: ''}{errors.includes(n.id) ? ', has errors' : ''}"
				onclick={() => select(n.id)}
				onkeydown={(e) => onkey(e, n.id)}
			>
				<rect width={NODE_W} height={NODE_H} rx="8" />
				{#if n.id === workflow.start}<circle cx="0" cy="0" r="5" class="ez-wf-node__start" />{/if}
				<text x="12" y="21" class="ez-wf-node__id">{clip(n.id, 20)}</text>
				<text x="12" y="39" class="ez-wf-node__sub">{clip(subtitle(n.id), 24)}</text>
			</g>
		{/each}
	</svg>
</div>

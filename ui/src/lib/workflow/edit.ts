/** Pure edits of a workflow {@link Definition} for the editor, plus the graph layout. */
import dagre from '@dagrejs/dagre';
import {
	ELSE,
	type Definition,
	type Edges,
	type NodeType,
	type WorkflowNode
} from './definition.ts';

/** A fresh node of `type` with the fields that type needs. */
export function newNode(id: string, type: NodeType): WorkflowNode {
	switch (type) {
		case 'command':
			return { id, type, run: '' };
		case 'agent':
			return { id, type, prompt: '' };
		case 'wait':
			return { id, type, events: ['comment'] };
		case 'action':
			return { id, type, action: 'post_comment' };
		case 'end':
			return { id, type, status: 'done' };
	}
}

/** `base`, or `base_2`, `base_3`, … : the first id not in `def`. */
export function freeId(def: Definition, base = 'step'): string {
	const ids = new Set(def.nodes.map((n) => n.id));
	if (!ids.has(base)) return base;
	for (let i = 2; ; i++) if (!ids.has(`${base}_${i}`)) return `${base}_${i}`;
}

/** Changes a node's type, keeping its id and edges, dropping fields of the old type. */
export function retype(n: WorkflowNode, type: NodeType): WorkflowNode {
	const fresh = newNode(n.id, type);
	if (type === 'end') return fresh;
	return {
		...fresh,
		next: n.next,
		max_visits: n.max_visits,
		on_limit: n.on_limit,
		optional: type === 'command' || type === 'agent' ? n.optional : undefined
	};
}

function mapTargets(n: WorkflowNode, f: (t: string) => string | null): void {
	if (n.next?.kind === 'to') {
		const t = f(n.next.to);
		n.next = t === null ? undefined : { kind: 'to', to: t };
	} else if (n.next?.kind === 'cond') {
		n.next = {
			kind: 'cond',
			conds: n.next.conds.flatMap((c) => {
				const t = f(c.to);
				return t === null ? [] : [{ ...c, to: t }];
			})
		};
	}
	if (n.on_limit !== undefined) {
		const t = f(n.on_limit);
		n.on_limit = t === null ? undefined : t;
	}
}

/** Renames a node and every reference to it (start, edges, on_limit). Mutates `def`. */
export function renameNode(def: Definition, from: string, to: string): void {
	if (from === to) return;
	for (const n of def.nodes) {
		if (n.id === from) n.id = to;
		mapTargets(n, (t) => (t === from ? to : t));
	}
	if (def.start === from) def.start = to;
}

/** Deletes a node and every edge to it. Mutates `def`. */
export function deleteNode(def: Definition, id: string): void {
	def.nodes = def.nodes.filter((n) => n.id !== id);
	for (const n of def.nodes) mapTargets(n, (t) => (t === id ? null : t));
}

/**
 * Connects `from` to `to`: sets a plain `next` when there is none, else adds a condition before
 * `else` (turning a plain `next` into `{<new>: to, else: <old>}`).
 */
export function connect(def: Definition, from: string, to: string): void {
	const n = def.nodes.find((x) => x.id === from);
	if (!n || n.type === 'end') return;
	if (!n.next) {
		n.next = { kind: 'to', to };
		return;
	}
	const conds = n.next.kind === 'to' ? [{ when: ELSE, to: n.next.to }] : n.next.conds;
	const at = conds.findIndex((c) => c.when === ELSE);
	const added = { when: 'ok', to };
	n.next = {
		kind: 'cond',
		conds: at < 0 ? [...conds, added] : [...conds.slice(0, at), added, ...conds.slice(at)]
	};
}

/** One drawn edge: its source, target and label (condition, `else`, `on_limit`). */
export interface DrawnEdge {
	id: string;
	source: string;
	target: string;
	label?: string;
	kind: 'next' | 'cond' | 'else' | 'limit';
}

export function drawnEdges(def: Definition): DrawnEdge[] {
	const ids = new Set(def.nodes.map((n) => n.id));
	const out: DrawnEdge[] = [];
	for (const n of def.nodes) {
		const e: Edges | undefined = n.next;
		if (e?.kind === 'to')
			out.push({ id: `${n.id}>next`, source: n.id, target: e.to, kind: 'next' });
		if (e?.kind === 'cond')
			e.conds.forEach((c, i) =>
				out.push({
					id: `${n.id}>${i}`,
					source: n.id,
					target: c.to,
					label: c.when === ELSE ? ELSE : `${i + 1}. ${c.when}`,
					kind: c.when === ELSE ? 'else' : 'cond'
				})
			);
		if (n.on_limit)
			out.push({
				id: `${n.id}>limit`,
				source: n.id,
				target: n.on_limit,
				label: 'on_limit',
				kind: 'limit'
			});
	}
	return out.filter((e) => ids.has(e.target));
}

export const NODE_W = 200;
export const NODE_H = 64;

/** Top-to-bottom layered layout (dagre). Positions are node top-left corners. */
export function layout(def: Definition): Map<string, { x: number; y: number }> {
	const g = new dagre.graphlib.Graph({ multigraph: true });
	g.setGraph({ rankdir: 'TB', nodesep: 40, ranksep: 70 });
	g.setDefaultEdgeLabel(() => ({}));
	for (const n of def.nodes) g.setNode(n.id, { width: NODE_W, height: NODE_H });
	for (const e of drawnEdges(def)) g.setEdge(e.source, e.target, {}, e.id);
	dagre.layout(g);
	const out = new Map<string, { x: number; y: number }>();
	for (const n of def.nodes) {
		const p = g.node(n.id);
		out.set(n.id, { x: p.x - NODE_W / 2, y: p.y - NODE_H / 2 });
	}
	return out;
}

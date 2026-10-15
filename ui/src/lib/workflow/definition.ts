/**
 * Graph workflow definitions (ADR-0020): parse, validate and serialize `.enzarb/workflows/*.yaml`.
 * `validate` is a port of `validate()` in `workflows/crates/graph/src/lib.rs` (same rules, same
 * messages); the shared fixture `workflows/crates/graph/tests/fixtures/validate.json` keeps the
 * two from drifting.
 *
 * One extension the Rust crate does not have yet: an agent `prompt` may be `{file: <path>}`
 * naming `.enzarb/prompts/<name>.md` (engine support is being added separately).
 */
import { Document, isMap, isScalar, isSeq, parseDocument, type Node as YamlNode } from 'yaml';
import { exprProblem } from './expr.ts';

export const VERSION = 1;
export const DEFAULT_MAX_VISITS = 5;
export const ELSE = 'else';
export const KNOWN_ACTIONS = [
	'post_comment',
	'open_change_request',
	'merge_change_request'
] as const;
export const MERGE_ACTION = 'merge_change_request';
export const KNOWN_EVENTS = [
	'comment',
	'approval',
	'ci',
	'review',
	'merge_requested',
	'dependencies_done',
	'timer'
] as const;
export const NODE_TYPES = ['command', 'agent', 'wait', 'action', 'end'] as const;
export const END_STATUSES = ['done', 'failed', 'cancelled'] as const;
export const WAIT_FROM = ['writers', 'anyone'] as const;
/** Named prompts the engine renders from its built-in templates (`named_prompt` in the engine). */
export const NAMED_PROMPTS = [
	'triage',
	'plan',
	'implement',
	'review',
	'address_comments',
	'resolve_conflicts',
	'task'
] as const;
/** Where prompt files live in a repository. */
export const PROMPT_DIR = '.enzarb/prompts/';
export const WORKFLOW_DIR = '.enzarb/workflows/';

export type NodeType = (typeof NODE_TYPES)[number];
export type EndStatus = (typeof END_STATUSES)[number];
export type WaitFrom = (typeof WAIT_FROM)[number];
export type Prompt = string | { file: string };

export type Edges = { kind: 'to'; to: string } | { kind: 'cond'; conds: Cond[] };
export interface Cond {
	when: string;
	to: string;
}

/** One node. Fields that do not apply to `type` are ignored by validation and serialization. */
export interface WorkflowNode {
	id: string;
	type: NodeType;
	run?: string;
	prompt?: Prompt;
	events?: string[];
	from?: WaitFrom;
	action?: string;
	body?: string;
	status?: EndStatus;
	next?: Edges;
	/** Unset means {@link DEFAULT_MAX_VISITS}. */
	max_visits?: number;
	on_limit?: string;
	optional?: boolean;
}

export interface Definition {
	version: number;
	name: string;
	on: string[];
	start: string;
	/** In file order. */
	nodes: WorkflowNode[];
}

export class ParseError extends Error {
	constructor(message: string) {
		super(message);
		this.name = 'ParseError';
	}
}

/** The prompt a named built-in, inline text or a file reference. */
export function promptKind(p: Prompt | undefined): 'named' | 'inline' | 'file' {
	if (p && typeof p === 'object') return 'file';
	return NAMED_PROMPTS.includes((p ?? '').trim() as (typeof NAMED_PROMPTS)[number])
		? 'named'
		: 'inline';
}

// --- parsing --------------------------------------------------------------------------------

const fail = (msg: string): never => {
	throw new ParseError(msg);
};

function scalarValue(n: unknown): unknown {
	return isScalar(n) ? n.value : n;
}

function str(n: unknown, field: string): string {
	const v = scalarValue(n);
	if (typeof v !== 'string') fail(`${field}: invalid type, expected a string`);
	return v as string;
}

function u32(n: unknown, field: string): number {
	const v = scalarValue(n);
	if (typeof v !== 'number' || !Number.isInteger(v) || v < 0 || v > 0xffffffff)
		fail(`${field}: invalid type, expected u32`);
	return v as number;
}

function bool(n: unknown, field: string): boolean {
	const v = scalarValue(n);
	if (typeof v !== 'boolean') fail(`${field}: invalid type, expected a boolean`);
	return v as boolean;
}

function strList(n: unknown, field: string): string[] {
	if (!isSeq(n)) fail(`${field}: invalid type, expected a sequence`);
	return (n as { items: unknown[] }).items.map((x, i) => str(x, `${field}[${i}]`));
}

function isNull(n: unknown): boolean {
	return n === undefined || n === null || (isScalar(n) && n.value === null);
}

/** A map's entries as `[key text, value node]`, in file order. */
function entries(n: unknown, field: string): [string, unknown][] {
	if (!isMap(n)) fail(`${field}: invalid type, expected a map`);
	return (n as { items: { key: unknown; value: unknown }[] }).items.map((p) => [
		String(scalarValue(p.key)),
		p.value
	]);
}

/** A condition key keeps its source text, as the Rust `CondKey` does for bools and integers. */
function condKey(k: unknown, field: string): string {
	const v = scalarValue(k);
	if (typeof v === 'string') return v;
	if (typeof v === 'boolean') return String(v);
	if (typeof v === 'number' && Number.isInteger(v)) return String(v);
	return fail(`${field}: invalid type, expected a condition`);
}

function parseEdges(n: unknown, field: string): Edges | undefined {
	if (isNull(n)) return undefined;
	if (isMap(n)) {
		return {
			kind: 'cond',
			conds: n.items.map((p, i) => ({
				when: condKey(p.key, `${field}[${i}]`),
				to: str(p.value, `${field}.${String(scalarValue(p.key))}`)
			}))
		};
	}
	const v = scalarValue(n);
	if (typeof v === 'string') return { kind: 'to', to: v };
	return fail(`${field}: invalid type, expected a node id or a map from condition to node id`);
}

function parsePrompt(n: unknown, field: string): Prompt {
	if (isMap(n)) {
		const e = Object.fromEntries(entries(n, field));
		if (!('file' in e)) fail(`${field}: missing field \`file\``);
		return { file: str(e.file, `${field}.file`) };
	}
	return str(n, field);
}

function oneOf<T extends string>(n: unknown, field: string, allowed: readonly T[]): T {
	const v = str(n, field);
	if (!allowed.includes(v as T))
		fail(`${field}: unknown variant \`${v}\`, expected one of ${allowed.join(', ')}`);
	return v as T;
}

function parseNode(id: string, n: unknown): WorkflowNode {
	const f = `nodes.${id}`;
	const e = new Map(entries(n, f));
	const need = (k: string) => {
		if (!e.has(k)) fail(`${f}: missing field \`${k}\``);
		return e.get(k);
	};
	const type = oneOf(need('type'), `${f}.type`, NODE_TYPES);
	const node: WorkflowNode = { id, type };
	switch (type) {
		case 'command':
			node.run = str(need('run'), `${f}.run`);
			break;
		case 'agent':
			node.prompt = parsePrompt(need('prompt'), `${f}.prompt`);
			break;
		case 'wait':
			node.events = strList(need('events'), `${f}.events`);
			if (e.has('from')) node.from = oneOf(e.get('from'), `${f}.from`, WAIT_FROM);
			break;
		case 'action':
			node.action = str(need('action'), `${f}.action`);
			if (e.has('body') && !isNull(e.get('body'))) node.body = str(e.get('body'), `${f}.body`);
			break;
		case 'end':
			node.status = oneOf(need('status'), `${f}.status`, END_STATUSES);
			break;
	}
	if (e.has('next')) node.next = parseEdges(e.get('next'), `${f}.next`);
	if (e.has('max_visits')) node.max_visits = u32(e.get('max_visits'), `${f}.max_visits`);
	if (e.has('on_limit') && !isNull(e.get('on_limit')))
		node.on_limit = str(e.get('on_limit'), `${f}.on_limit`);
	if (e.has('optional')) node.optional = bool(e.get('optional'), `${f}.optional`);
	return node;
}

/** Parses YAML into a document and a model. Throws {@link ParseError}. */
export function parseWorkflow(src: string): { doc: Document; def: Definition } {
	const doc = parseDocument(src, { uniqueKeys: false, prettyErrors: false });
	if (doc.errors.length) fail(doc.errors[0].message);
	const root = doc.contents;
	const top = new Map(entries(root, 'definition'));
	const need = (k: string) => {
		if (!top.has(k)) fail(`missing field \`${k}\``);
		return top.get(k);
	};
	const version = u32(need('version'), 'version');
	const name = str(need('name'), 'name');
	const on = top.has('on') && !isNull(top.get('on')) ? strList(top.get('on'), 'on') : [];
	const start = str(need('start'), 'start');
	// A repeated node id keeps the last node, as the Rust map does.
	const byId = new Map<string, WorkflowNode>();
	for (const [id, n] of entries(need('nodes'), 'nodes')) byId.set(id, parseNode(id, n));
	return { doc, def: { version, name, on, start, nodes: [...byId.values()] } };
}

// --- validation -----------------------------------------------------------------------------

function validId(id: string): boolean {
	return /^[a-z0-9_-]{1,64}$/.test(id);
}

function promptText(p: Prompt | undefined): string {
	if (p && typeof p === 'object') return p.file;
	return p ?? '';
}

/** Every edge target of `n`: `next` targets in order, then `on_limit`. */
export function edgesOf(n: WorkflowNode): string[] {
	const out: string[] = [];
	if (n.next?.kind === 'to') out.push(n.next.to);
	if (n.next?.kind === 'cond') out.push(...n.next.conds.map((c) => c.to));
	if (n.on_limit !== undefined) out.push(n.on_limit);
	return out;
}

/** A problem, with the node it is about when there is one. */
export interface Problem {
	node?: string;
	message: string;
}

const byKey = (a: WorkflowNode, b: WorkflowNode) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0);

/** Every rule of ADR-0020 §1, in the order the Rust `validate` reports them. */
export function validate(d: Definition): Problem[] {
	const p: Problem[] = [];
	const push = (message: string, node?: string) => p.push({ node, message });
	// The Rust model is a BTreeMap: iterate sorted, and a duplicate id keeps the last node.
	const map = new Map<string, WorkflowNode>();
	for (const n of d.nodes) map.set(n.id, n);
	const nodes = [...map.values()].sort(byKey);
	if (d.version !== VERSION) push(`unsupported version ${d.version} (expected ${VERSION})`);
	if (d.name.trim() === '') push('`name` is empty');
	if (d.on.length === 0) push('`on` lists no triggers');
	if (nodes.length === 0) push('no nodes');
	if (!map.has(d.start)) push(`start node \`${d.start}\` does not exist`);
	for (const n of nodes) {
		const id = n.id;
		if (!validId(id)) push(`node id \`${id}\` must match [a-z0-9_-]{1,64}`, id);
		if (n.type === 'command' && (n.run ?? '').trim() === '')
			push(`node \`${id}\`: \`run\` is empty`, id);
		if (n.type === 'agent') {
			if (promptText(n.prompt).trim() === '') push(`node \`${id}\`: \`prompt\` is empty`, id);
			else if (typeof n.prompt === 'object' && !isPromptPath(n.prompt.file))
				push(`node \`${id}\`: prompt file \`${n.prompt.file}\` must be ${PROMPT_DIR}<name>.md`, id);
		}
		if (n.type === 'wait') {
			const events = n.events ?? [];
			if (events.length === 0) push(`node \`${id}\`: \`events\` is empty`, id);
			for (const e of events)
				if (!(KNOWN_EVENTS as readonly string[]).includes(e))
					push(`node \`${id}\`: unknown event \`${e}\``, id);
		}
		if (n.type === 'action' && !(KNOWN_ACTIONS as readonly string[]).includes(n.action ?? ''))
			push(`node \`${id}\`: unknown action \`${n.action ?? ''}\``, id);
		const isEnd = n.type === 'end';
		if (n.next && isEnd) push(`node \`${id}\`: an end node has no \`next\``, id);
		if (!n.next && !isEnd) push(`node \`${id}\`: \`next\` is missing`, id);
		if ((n.max_visits ?? DEFAULT_MAX_VISITS) === 0)
			push(`node \`${id}\`: \`max_visits\` must be at least 1`, id);
		if (n.optional && n.type !== 'command' && n.type !== 'agent')
			push(`node \`${id}\`: only command and agent nodes can be optional`, id);
		if (n.next?.kind === 'cond') {
			const c = n.next.conds;
			if (c.length === 0) push(`node \`${id}\`: \`next\` has no conditions`, id);
			c.forEach(({ when }, i) => {
				if (when === ELSE) {
					if (i + 1 !== c.length) push(`node \`${id}\`: \`else\` must be the last condition`, id);
				} else {
					const e = exprProblem(when);
					if (e) push(`node \`${id}\`: ${e}`, id);
				}
			});
			if (c.length > 0 && c[c.length - 1].when !== ELSE)
				push(`node \`${id}\`: conditional \`next\` needs an \`else\``, id);
		}
		for (const t of edgesOf(n))
			if (!map.has(t)) push(`node \`${id}\`: edge to unknown node \`${t}\``, id);
		if (n.on_limit === id) push(`node \`${id}\`: \`on_limit\` points at itself`, id);
	}
	// Every node can reach an end: walk the edges backwards from the ends.
	const canEnd = new Set(nodes.filter((n) => n.type === 'end').map((n) => n.id));
	const queue = [...canEnd].sort();
	while (queue.length) {
		const t = queue.shift()!;
		for (const n of nodes)
			if (!canEnd.has(n.id) && edgesOf(n).includes(t)) {
				canEnd.add(n.id);
				queue.push(n.id);
			}
	}
	for (const n of nodes)
		if (!canEnd.has(n.id)) push(`node \`${n.id}\` cannot reach an end node`, n.id);
	// The merge gate: a merge is entered only from a wait on exactly `merge_requested`.
	for (const m of nodes) {
		if (!(m.type === 'action' && m.action === MERGE_ACTION)) continue;
		if (m.id === d.start) push(`node \`${m.id}\`: a merge cannot be the start node`, m.id);
		for (const n of nodes) {
			if (!edgesOf(n).includes(m.id)) continue;
			const events = n.events ?? [];
			const gated =
				n.type === 'wait' &&
				events.length > 0 &&
				events.every((e) => e === 'merge_requested') &&
				n.on_limit !== m.id;
			if (!gated)
				push(
					`node \`${n.id}\`: \`${MERGE_ACTION}\` (\`${m.id}\`) is reachable only through a wait on \`merge_requested\``,
					n.id
				);
		}
	}
	return p;
}

export function isPromptPath(path: string): boolean {
	return /^\.enzarb\/prompts\/[A-Za-z0-9_-][A-Za-z0-9_.-]*\.md$/.test(path) && !path.includes('..');
}

/** Parse and validate YAML. Messages match the Rust `load` problems. */
export function check(src: string): { parse?: string; problems: Problem[]; def?: Definition } {
	try {
		const { def } = parseWorkflow(src);
		return { problems: validate(def), def };
	} catch (e) {
		if (e instanceof ParseError) return { parse: e.message, problems: [] };
		throw e;
	}
}

// --- serialization --------------------------------------------------------------------------

function edgesValue(doc: Document, e: Edges): unknown {
	if (e.kind === 'to') return e.to;
	const m = doc.createNode({}) as ReturnType<Document['createNode']> & {
		items: unknown[];
		flow?: boolean;
	};
	for (const c of e.conds)
		(m as unknown as { add: (p: unknown) => void }).add(doc.createPair(c.when, c.to));
	return m;
}

/** The YAML fields of a node, in canonical order, without defaults. */
function nodeFields(n: WorkflowNode): [string, unknown][] {
	const out: [string, unknown][] = [['type', n.type]];
	switch (n.type) {
		case 'command':
			out.push(['run', n.run ?? '']);
			break;
		case 'agent':
			out.push([
				'prompt',
				typeof n.prompt === 'object' ? { file: n.prompt.file } : (n.prompt ?? '')
			]);
			break;
		case 'wait':
			out.push(['events', [...(n.events ?? [])]]);
			if (n.from && n.from !== 'writers') out.push(['from', n.from]);
			break;
		case 'action':
			out.push(['action', n.action ?? '']);
			if (n.body !== undefined && n.action === 'post_comment') out.push(['body', n.body]);
			break;
		case 'end':
			out.push(['status', n.status ?? 'done']);
			break;
	}
	if (n.optional) out.push(['optional', true]);
	if (n.next && n.type !== 'end') out.push(['next', n.next]);
	if (n.max_visits !== undefined && n.max_visits !== DEFAULT_MAX_VISITS)
		out.push(['max_visits', n.max_visits]);
	if (n.on_limit) out.push(['on_limit', n.on_limit]);
	return out;
}

function sameJs(a: unknown, b: unknown): boolean {
	return JSON.stringify(a) === JSON.stringify(b);
}

/** The JS form of a model field, for comparing with what a document holds. */
function fieldJs(k: string, v: unknown): unknown {
	if (k !== 'next') return v;
	const e = v as Edges;
	return e.kind === 'to' ? e.to : e.conds.map((c) => [c.when, c.to]);
}

function docFieldJs(k: string, n: YamlNode | undefined): unknown {
	if (k === 'next' && isMap(n))
		return n.items.map((p) => [condKey(p.key, 'next'), scalarValue(p.value)]);
	return (n as { toJSON?: () => unknown } | undefined)?.toJSON?.() ?? scalarValue(n);
}

/**
 * Writes `def` into `doc` in place, touching only what changed, so comments, key order and flow
 * style of untouched parts survive. Returns the YAML text.
 */
export function applyToDocument(doc: Document, def: Definition, src?: string): string {
	// Unchanged: keep the source byte for byte (the printer would normalize spacing).
	if (src !== undefined) {
		try {
			if (sameJs(parseWorkflow(src).def, def)) return src;
		} catch {
			// The source does not parse: print the model.
		}
	}
	if (!isMap(doc.contents)) doc.contents = doc.createNode({}) as typeof doc.contents;
	const set = (k: string, v: unknown) => {
		if (!sameJs(docFieldJs(k, doc.get(k, true) as YamlNode | undefined), v)) doc.set(k, v);
	};
	set('version', def.version);
	set('name', def.name);
	if (!sameJs(docFieldJs('on', doc.get('on', true) as YamlNode | undefined), def.on)) {
		const seq = doc.createNode([...def.on]) as { flow?: boolean };
		seq.flow = true;
		doc.set('on', seq);
	}
	set('start', def.start);
	let nodes = doc.get('nodes', true);
	if (!isMap(nodes)) {
		nodes = doc.createNode({});
		doc.set('nodes', nodes);
	}
	const nmap = nodes as unknown as {
		items: { key: unknown; value: unknown }[];
		get: (k: string, keep: true) => unknown;
		set: (k: string, v: unknown) => void;
		delete: (k: string) => boolean;
		add: (p: unknown) => void;
	};
	const wanted = new Set(def.nodes.map((n) => n.id));
	for (const p of [...nmap.items]) {
		const k = String(scalarValue(p.key));
		if (!wanted.has(k)) nmap.delete(k);
	}
	for (const n of def.nodes) {
		const fields = nodeFields(n);
		const cur = nmap.get(n.id, true);
		if (!isMap(cur)) {
			const fresh = doc.createNode({}) as unknown as { flow: boolean; add: (p: unknown) => void };
			for (const [k, v] of fields)
				fresh.add(doc.createPair(k, k === 'next' ? edgesValue(doc, v as Edges) : v));
			fresh.flow = true;
			nmap.set(n.id, fresh);
			continue;
		}
		const keep = new Set(fields.map(([k]) => k));
		for (const p of [...cur.items]) {
			const k = String(scalarValue(p.key));
			if (!keep.has(k)) cur.delete(k);
		}
		for (const [k, v] of fields) {
			const have = cur.get(k, true) as YamlNode | undefined;
			if (have !== undefined && sameJs(docFieldJs(k, have), fieldJs(k, v))) continue;
			let nv: unknown = k === 'next' ? edgesValue(doc, v as Edges) : v;
			if (k === 'next' && isMap(nv) && isMap(have)) (nv as { flow?: boolean }).flow = have.flow;
			else if (k === 'next' && isMap(nv)) (nv as { flow?: boolean }).flow = !!cur.flow;
			if (Array.isArray(nv) || (nv && typeof nv === 'object' && !isMap(nv))) {
				const created = doc.createNode(nv) as { flow?: boolean };
				created.flow = true;
				nv = created;
			}
			cur.set(k, nv);
		}
	}
	// Model order wins for nodes (new nodes go where the editor put them).
	const order = new Map(def.nodes.map((n, i) => [n.id, i]));
	nmap.items.sort(
		(a, b) =>
			(order.get(String(scalarValue(a.key))) ?? 0) - (order.get(String(scalarValue(b.key))) ?? 0)
	);
	return doc.toString({ lineWidth: 0, flowCollectionPadding: false });
}

/** A fresh YAML text for `def` (no source to preserve). */
export function serialize(def: Definition): string {
	return applyToDocument(new Document({}), def);
}

/**
 * `.ez/workflows.toml` for the workflow designer (ADR-0029 §2). The wire types are ez-workflow's
 * serde shapes as enzarb-tracks returns them (snake_case, `if` for conditions); the designer edits
 * a flat, array-based document and converts at the boundary. Validation is tracks' alone.
 */

export type NodeType = 'agent' | 'command' | 'action' | 'wait' | 'end';
export type ActionKind = 'comment' | 'open_pull_request' | 'add_labels' | 'remove_labels';
export type EndStatus = 'done' | 'failed' | 'cancelled';

export const NODE_TYPES: readonly NodeType[] = ['agent', 'command', 'action', 'wait', 'end'];
export const ACTION_KINDS: readonly ActionKind[] = [
	'comment',
	'open_pull_request',
	'add_labels',
	'remove_labels'
];
export const END_STATUSES: readonly EndStatus[] = ['done', 'failed', 'cancelled'];

/** Output names per node type (ez-workflow `NodeType::outputs`). */
export const NODE_OUTPUTS: Record<NodeType, readonly string[]> = {
	agent: ['result', 'git', 'exit'],
	command: ['exit_code', 'output', 'json'],
	action: ['number', 'url'],
	wait: ['event', 'timed_out'],
	end: []
};

// ----- wire -----

export type VarValue = string | number | boolean;

export interface EdgeWire {
	if?: string | null;
	to: string;
}
export type NextWire = string | EdgeWire[];

interface Visits {
	next: NextWire;
	max_visits?: number | null;
}
export type NodeWire =
	| ({
			type: 'agent';
			mode: string;
			instruction: string;
			result?: Record<string, unknown> | null;
			push?: boolean;
	  } & Visits)
	| ({ type: 'command'; task?: string | null; args?: string[]; run?: string | null } & Visits)
	| ({
			type: 'action';
			action: ActionKind;
			title?: string | null;
			body?: string | null;
			labels?: string[];
			draft?: boolean | null;
	  } & Visits)
	| ({ type: 'wait'; on: string[]; if?: string | null; timeout?: string | null } & Visits)
	| { type: 'end'; status: EndStatus; reason?: string | null };

export interface WorkflowWire {
	description?: string;
	on: string[];
	if?: string | null;
	start: string;
	timeout?: string | null;
	max_runs?: number | null;
	/** Scalar settings, readable as `vars.<name>` in templates and conditions. */
	vars?: Record<string, VarValue>;
	nodes: Record<string, NodeWire>;
}

export interface WorkflowsFileWire {
	version: number;
	workflows?: Record<string, WorkflowWire>;
}

export interface Diagnostic {
	severity: 'error' | 'warning';
	/** Dotted TOML path (`workflows.fix.nodes.triage.mode`); empty for the whole file. */
	path: string;
	message: string;
}

// ----- designer document -----

export interface EdgeDoc {
	/** Empty: unconditional. */
	if: string;
	to: string;
}

/** Every field of every node type; `toWire` keeps the ones of `type`, so switching types is lossless. */
export interface NodeDoc {
	id: string;
	type: NodeType;
	mode: string;
	instruction: string;
	/** `submit_result` properties: name → JSON Schema fragment; null: no result. */
	result: Record<string, unknown> | null;
	push: boolean;
	task: string;
	args: string[];
	run: string;
	action: ActionKind;
	title: string;
	body: string;
	labels: string[];
	/** null: the default (not a draft). */
	draft: boolean | null;
	on: string[];
	if: string;
	timeout: string;
	status: EndStatus;
	/** `end` nodes: a template saying why the run ends here; empty: a generic reason. */
	reason: string;
	next: EdgeDoc[];
	max_visits: number | null;
}

export interface WorkflowDoc {
	id: string;
	description: string;
	on: string[];
	if: string;
	start: string;
	timeout: string;
	max_runs: number | null;
	/** `[workflows.<id>.vars]`; carried through the designer untouched. */
	vars?: Record<string, VarValue>;
	nodes: NodeDoc[];
}

export interface WorkflowsDoc {
	workflows: WorkflowDoc[];
}

/** One repository's `.ez/workflows.toml` as the designer opens it. */
export interface WorkflowsFile {
	/** `github:owner/repo`. */
	repo: string;
	branch: string;
	path: string;
	/** Blob sha of the loaded file (null: no file yet); sent back on save. */
	sha: string | null;
	text: string | null;
	/** The parsed file; null when it does not parse (only the TOML editor can fix it). */
	file: WorkflowsFileWire | null;
	report: Diagnostic[];
	/** `.ez/modes.toml` on the same branch (null: none), sent along to checks and dry runs. */
	modesToml: string | null;
	/** Mode ids of that file, for the agent node's mode picker. */
	modeIds: string[];
}

export function blankNode(id: string, type: NodeType = 'agent'): NodeDoc {
	return {
		id,
		type,
		mode: '',
		instruction: '',
		result: null,
		push: false,
		task: '',
		args: [],
		run: '',
		action: 'comment',
		title: '',
		body: '',
		labels: [],
		draft: null,
		on: [],
		if: '',
		timeout: '',
		status: 'done',
		reason: '',
		next: [],
		max_visits: null
	};
}

export function blankWorkflow(id: string): WorkflowDoc {
	const done = blankNode('done', 'end');
	const first = { ...blankNode('start'), next: [{ if: '', to: 'done' }] };
	return {
		id,
		description: '',
		on: ['issues.labeled'],
		if: '',
		start: 'start',
		timeout: '',
		max_runs: null,
		nodes: [first, done]
	};
}

const edgesFrom = (next: NextWire): EdgeDoc[] =>
	typeof next === 'string'
		? [{ if: '', to: next }]
		: next.map((e) => ({ if: e.if ?? '', to: e.to }));

function nodeFromWire(id: string, n: NodeWire): NodeDoc {
	const d = blankNode(id, n.type);
	switch (n.type) {
		case 'agent':
			Object.assign(d, {
				mode: n.mode,
				instruction: n.instruction,
				result: n.result ?? null,
				push: n.push ?? false
			});
			break;
		case 'command':
			Object.assign(d, { task: n.task ?? '', args: n.args ?? [], run: n.run ?? '' });
			break;
		case 'action':
			Object.assign(d, {
				action: n.action,
				title: n.title ?? '',
				body: n.body ?? '',
				labels: n.labels ?? [],
				draft: n.draft ?? null
			});
			break;
		case 'wait':
			Object.assign(d, { on: n.on, if: n.if ?? '', timeout: n.timeout ?? '' });
			break;
		case 'end':
			d.status = n.status;
			d.reason = n.reason ?? '';
			return d;
	}
	d.next = edgesFrom(n.next);
	d.max_visits = n.max_visits ?? null;
	return d;
}

export function fromWire(file: WorkflowsFileWire): WorkflowsDoc {
	return {
		workflows: Object.entries(file.workflows ?? {}).map(([id, w]) => ({
			id,
			description: w.description ?? '',
			on: [...w.on],
			if: w.if ?? '',
			start: w.start,
			timeout: w.timeout ?? '',
			max_runs: w.max_runs ?? null,
			...(w.vars && Object.keys(w.vars).length ? { vars: { ...w.vars } } : {}),
			nodes: Object.entries(w.nodes).map(([nid, n]) => nodeFromWire(nid, n))
		}))
	};
}

/** Puts `key` on `o` only when the value is set (serde rejects `null` for lists and booleans). */
function opt<T extends object>(o: T, key: string, value: unknown): T {
	const empty =
		value === null ||
		value === undefined ||
		value === '' ||
		(Array.isArray(value) && value.length === 0);
	if (!empty) (o as Record<string, unknown>)[key] = value;
	return o;
}

function nextToWire(next: EdgeDoc[]): NextWire {
	if (next.length === 1 && !next[0].if.trim()) return next[0].to;
	return next.map((e) => (e.if.trim() ? { if: e.if, to: e.to } : { to: e.to }));
}

function nodeToWire(n: NodeDoc): NodeWire {
	if (n.type === 'end') return opt({ type: 'end', status: n.status }, 'reason', n.reason);
	const tail = (o: object) => {
		(o as Record<string, unknown>).next = nextToWire(n.next);
		return opt(o, 'max_visits', n.max_visits);
	};
	switch (n.type) {
		case 'agent': {
			const o: Record<string, unknown> = {
				type: 'agent',
				mode: n.mode,
				instruction: n.instruction
			};
			if (n.result && Object.keys(n.result).length) o.result = n.result;
			if (n.push) o.push = true;
			return tail(o) as NodeWire;
		}
		case 'command': {
			const o = { type: 'command' };
			opt(o, 'task', n.task);
			opt(o, 'args', n.args);
			opt(o, 'run', n.run);
			return tail(o) as NodeWire;
		}
		case 'action': {
			const o = { type: 'action', action: n.action };
			opt(o, 'title', n.title);
			opt(o, 'body', n.body);
			opt(o, 'labels', n.labels);
			opt(o, 'draft', n.draft);
			return tail(o) as NodeWire;
		}
		case 'wait': {
			const o = { type: 'wait', on: n.on };
			opt(o, 'if', n.if);
			opt(o, 'timeout', n.timeout);
			return tail(o) as NodeWire;
		}
	}
}

export function toWire(doc: WorkflowsDoc): WorkflowsFileWire {
	const workflows: Record<string, WorkflowWire> = {};
	for (const w of doc.workflows) {
		const nodes: Record<string, NodeWire> = {};
		for (const n of w.nodes) nodes[n.id] = nodeToWire(n);
		const o = { on: w.on, start: w.start, nodes } as WorkflowWire;
		opt(o, 'description', w.description);
		opt(o, 'if', w.if);
		opt(o, 'timeout', w.timeout);
		opt(o, 'max_runs', w.max_runs);
		if (w.vars && Object.keys(w.vars).length) o.vars = { ...w.vars };
		workflows[w.id] = o;
	}
	return { version: 1, workflows };
}

// ----- editing helpers -----

/** First free id `base`, `base-2`, `base-3`, ... among `taken`. */
export function freeId(taken: readonly string[], base: string): string {
	const stem = base.replace(/-\d+$/, '') || 'node';
	if (!taken.includes(stem)) return stem;
	for (let i = 2; ; i++) if (!taken.includes(`${stem}-${i}`)) return `${stem}-${i}`;
}

/** Renames a node and every reference to it (`start`, edges). */
export function renameNode(wf: WorkflowDoc, from: string, to: string): WorkflowDoc {
	return {
		...wf,
		start: wf.start === from ? to : wf.start,
		nodes: wf.nodes.map((n) => ({
			...n,
			id: n.id === from ? to : n.id,
			next: n.next.map((e) => (e.to === from ? { ...e, to } : e))
		}))
	};
}

/** Removes a node and the edges into it. */
export function removeNode(wf: WorkflowDoc, id: string): WorkflowDoc {
	return {
		...wf,
		nodes: wf.nodes
			.filter((n) => n.id !== id)
			.map((n) => ({ ...n, next: n.next.filter((e) => e.to !== id) }))
	};
}

/** Where a diagnostic points in the document, from its dotted path. */
export function diagnosticTarget(path: string): { workflow: string; node: string | null } | null {
	const parts = path.split('.').map((p) => p.replace(/\[\d+\]$/, ''));
	if (parts[0] !== 'workflows' || !parts[1]) return null;
	return { workflow: parts[1], node: parts[2] === 'nodes' && parts[3] ? parts[3] : null };
}

export const hasErrorDiagnostics = (d: readonly Diagnostic[]) =>
	d.some((x) => x.severity === 'error');

// ----- graph layout -----

export const NODE_W = 168;
export const NODE_H = 52;
const GAP_X = 40;
const GAP_Y = 64;
export const PAD = 24;

export interface LaidNode {
	id: string;
	x: number;
	y: number;
	layer: number;
	/** Not reachable from `start`. */
	orphan: boolean;
}
export interface LaidEdge {
	from: string;
	to: string;
	/** Position in `from`'s `next`. */
	index: number;
	label: string;
	/** Goes to the same or an earlier layer (a loop): drawn around the side. */
	back: boolean;
	/** Points at a node that does not exist. */
	dangling: boolean;
}
export interface Layout {
	nodes: LaidNode[];
	edges: LaidEdge[];
	width: number;
	height: number;
}

/**
 * Layered top-down layout: a node's layer is its breadth-first distance from `start`; nodes the
 * start cannot reach go in a last layer. Positions are derived, never stored in the file.
 */
export function layoutWorkflow(wf: WorkflowDoc): Layout {
	const byId = new Map(wf.nodes.map((n) => [n.id, n]));
	const layer = new Map<string, number>();
	const order: string[] = [];
	if (byId.has(wf.start)) {
		layer.set(wf.start, 0);
		const queue = [wf.start];
		while (queue.length) {
			const id = queue.shift()!;
			order.push(id);
			for (const e of byId.get(id)!.next) {
				if (byId.has(e.to) && !layer.has(e.to)) {
					layer.set(e.to, layer.get(id)! + 1);
					queue.push(e.to);
				}
			}
		}
	}
	const reached = Math.max(-1, ...layer.values());
	for (const n of wf.nodes) {
		if (!layer.has(n.id)) {
			layer.set(n.id, reached + 1);
			order.push(n.id);
		}
	}
	const rows: string[][] = [];
	for (const id of order) (rows[layer.get(id)!] ??= []).push(id);
	const widest = Math.max(1, ...rows.map((r) => r.length));
	const width = PAD * 2 + widest * NODE_W + (widest - 1) * GAP_X + NODE_W / 2;
	const nodes: LaidNode[] = [];
	rows.forEach((row, l) => {
		const rowW = row.length * NODE_W + (row.length - 1) * GAP_X;
		const x0 = PAD + (widest * NODE_W + (widest - 1) * GAP_X - rowW) / 2;
		row.forEach((id, i) =>
			nodes.push({
				id,
				x: x0 + i * (NODE_W + GAP_X),
				y: PAD + l * (NODE_H + GAP_Y),
				layer: l,
				orphan: l > reached
			})
		);
	});
	const edges: LaidEdge[] = [];
	for (const n of wf.nodes) {
		n.next.forEach((e, index) => {
			const target = layer.get(e.to);
			edges.push({
				from: n.id,
				to: e.to,
				index,
				label: e.if.trim() || (n.next.length > 1 ? 'else' : ''),
				back: target !== undefined && target <= layer.get(n.id)!,
				dangling: target === undefined
			});
		});
	}
	const height = PAD * 2 + rows.length * NODE_H + Math.max(0, rows.length - 1) * GAP_Y;
	return { nodes, edges, width, height: Math.max(height, NODE_H + PAD * 2) };
}

// ----- dry run -----

export interface DryRunEventRef {
	id: number;
	event: string;
	action: string | null;
	summary: string;
	receivedAt: string;
}

export interface TriggerEval {
	workflow: string;
	matched: boolean;
	on?: string | null;
	reason: string;
}

export type Transition =
	| { kind: 'moved'; from: string; to: string; edge: number }
	| { kind: 'ended'; status: EndStatus; reason?: string | null };

export interface TraceStep {
	node: string;
	node_type: NodeType;
	visit: number;
	/** The node with its templates rendered (ez-workflow `Rendered`, tagged by `type`). */
	rendered?: { type: NodeType } & Record<string, unknown>;
	mode?: { id: string; tools: string[] | null };
	assumed?: unknown;
	transition?: Transition;
}

export type TraceEnd =
	| { kind: 'ended'; status: EndStatus; node?: string; reason?: string }
	| {
			kind: 'needs_outcome';
			node: string;
			node_type: NodeType;
			outputs: string[];
			edges: EdgeWire[];
	  }
	| { kind: 'step_limit'; steps: number };

export interface Trace {
	workflow: string;
	forced: boolean;
	steps: TraceStep[];
	end: TraceEnd;
}

export interface DryRunResult {
	event_id: number;
	triggers: TriggerEval[];
	traces: Trace[];
}

export interface DryRunOptions {
	workflow?: string;
	assume?: Record<string, unknown>;
	max_steps?: number;
}

export interface DryRunResponse {
	report: { diagnostics: Diagnostic[] };
	results: DryRunResult[];
}

function sampleFor(schema: unknown): unknown {
	const s = (schema ?? {}) as { enum?: unknown[]; type?: unknown };
	if (Array.isArray(s.enum) && s.enum.length) return s.enum[0];
	switch (s.type) {
		case 'string':
			return '';
		case 'number':
		case 'integer':
			return 0;
		case 'boolean':
			return true;
		case 'array':
			return [];
		case 'object':
			return {};
		default:
			return null;
	}
}

/** A starting point for the outputs a dry run should assume for a node (edited as JSON). */
export function assumeTemplate(node: NodeDoc | undefined, type: NodeType): Record<string, unknown> {
	switch (type) {
		case 'agent': {
			const result: Record<string, unknown> = {};
			for (const [k, v] of Object.entries(node?.result ?? {})) result[k] = sampleFor(v);
			return {
				result,
				git: { branch: 'ez-test/dry-run', head_sha: '0000000', pushed: true },
				exit: 'end_turn'
			};
		}
		case 'command':
			return { exit_code: 0, output: '', json: null };
		case 'action':
			return { number: 1, url: 'https://github.com/owner/repo/issues/1' };
		case 'wait':
			return { event: {}, timed_out: false };
		case 'end':
			return {};
	}
}

/** Messages of the diagnostics at `path` or inside it (`path[0]`, `path.x`). */
export function diagnosticsAt(diags: readonly Diagnostic[], path: string): string[] {
	return diags
		.filter(
			(d) => d.path === path || d.path.startsWith(`${path}[`) || d.path.startsWith(`${path}.`)
		)
		.map((d) => d.message);
}

/** Node ids of `workflow` with at least one error. */
export function nodesWithErrors(diags: readonly Diagnostic[], workflow: string): string[] {
	const out: string[] = [];
	for (const d of diags) {
		const t = d.severity === 'error' ? diagnosticTarget(d.path) : null;
		if (t?.workflow === workflow && t.node && !out.includes(t.node)) out.push(t.node);
	}
	return out;
}

/** The ADR-0029 example: triage an issue labelled `ez`, then fix it or ask for details. */
export function exampleWorkflow(id: string): WorkflowDoc {
	const issue = '#{{ event.issue.number }}';
	return fromWire({
		version: 1,
		workflows: {
			[id]: {
				description: 'Fix issues labelled ez and open a pull request',
				on: ['issues.labeled'],
				if: "event.label.name == 'ez'",
				start: 'triage',
				nodes: {
					triage: {
						type: 'agent',
						mode: 'triage',
						instruction: `Issue ${issue}: {{ event.issue.title }}\n\n{{ event.issue.body }}`,
						result: {
							decision: { enum: ['fix', 'needs_info'] },
							summary: { type: 'string' }
						},
						next: [{ if: "result.decision == 'fix'", to: 'implement' }, { to: 'ask' }]
					},
					ask: {
						type: 'action',
						action: 'comment',
						body: '{{ nodes.triage.result.summary }}',
						next: 'wait_reply'
					},
					wait_reply: {
						type: 'wait',
						on: ['issue_comment.created'],
						if: 'event.comment.user.login == subject.author',
						timeout: '7d',
						next: [{ if: 'timed_out', to: 'end_stale' }, { to: 'triage' }]
					},
					implement: {
						type: 'agent',
						mode: 'implement',
						instruction: `Fix issue ${issue}. Plan: {{ nodes.triage.result.summary }}`,
						push: true,
						next: [{ if: 'git.pushed', to: 'open_pr' }, { to: 'end_failed' }]
					},
					open_pr: {
						type: 'action',
						action: 'open_pull_request',
						title: `Fix ${issue}: {{ event.issue.title }}`,
						body: `Closes ${issue}`,
						next: 'end_done'
					},
					end_done: { type: 'end', status: 'done' },
					end_failed: { type: 'end', status: 'failed' },
					end_stale: { type: 'end', status: 'cancelled' }
				}
			}
		}
	}).workflows[0];
}

/** Most events one dry run takes. */
export const MAX_DRY_RUN_EVENTS = 50;

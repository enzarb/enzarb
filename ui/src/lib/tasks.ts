import * as v from 'valibot';

/**
 * Types of the engine's projection and read API (`engine_api`, docs/workflows/contracts-v2.md).
 * The UI renders these; it never derives a task's state, column, buttons or "needs you" itself.
 */

export interface Subject {
	kind: string;
	key: string;
	url?: string | null;
	title?: string | null;
}

/** A declared wait button: submitting it posts `/command` as the user (`submit` kind `command`). */
export interface UiAction {
	label: string;
	command: string;
}

/**
 * A command that applies to the task now (`task_v.commands`): posting `/name` as the user does
 * what it says. `from: 'writer'` ones need write access to the repository.
 */
export interface TaskCommand {
	name: string;
	label: string;
	description?: string | null;
	from: 'writer' | 'anyone';
}

export interface ArtifactRef {
	title: string;
	format: string;
	node?: string;
}

export interface Artifact extends ArtifactRef {
	content: string;
	/** Sanitized HTML of a `markdown` artifact, rendered server-side. */
	html?: string;
}

/** One `engine_api.task_v` row. */
export interface TaskRow {
	id: number;
	project: string;
	repo: string | null;
	workflowRef: string | null;
	title: string;
	subjects: Subject[];
	/** Engine phase: `running`, `waiting`, `paused` or `ended`. */
	status: string;
	endStatus: string | null;
	phase: string;
	node: string;
	column: string | null;
	waitingOn: string | null;
	reason: string | null;
	text: string | null;
	actions: UiAction[];
	/** Additive: absent (empty) from an older engine, whose `actions` are all there is. */
	commands: TaskCommand[];
	artifacts: ArtifactRef[];
	needsYou: boolean;
	costUsd: number | null;
	createdAt: string;
	updatedAt: string;
	endedAt: string | null;
}

export interface TransitionSummary {
	at: string;
	eventType: string | null;
	from: { status: string | null; node: string | null };
	to: { status: string | null; node: string | null };
	effects: number;
}

export interface StepRun {
	runId: string;
	node: string;
	step: string;
	attempt: number;
	runtime: string | null;
	model: string | null;
	startedAt: string | null;
	endedAt: string | null;
	activeSecs: number | null;
	queueSecs: number | null;
	exit: string | null;
	tokens: Record<string, number> | null;
	costUsd: number | null;
	resumes: number;
	stalls: number;
	steers: number;
}

export interface TaskDetail extends Omit<TaskRow, 'artifacts'> {
	artifacts: Artifact[];
	transitions: TransitionSummary[];
	stepRuns: StepRun[];
}

export const SESSION_KINDS = [
	'prompt',
	'user',
	'assistant',
	'thinking',
	'tool_call',
	'tool_result',
	'usage',
	'steer',
	'system',
	'stdout',
	'stderr'
] as const;
export type SessionKind = (typeof SESSION_KINDS)[number];

export interface SessionEvent {
	seq: number;
	at: string;
	kind: string;
	payload: unknown;
	truncated: boolean;
}

export interface SessionHit {
	runId: string;
	taskId: number;
	seq: number;
	at: string;
	kind: string;
	snippet: string;
}

export interface BoardColumn {
	id: string;
	title: string;
	limit?: number | null;
}

export interface Board {
	columns: BoardColumn[];
}

export interface Origin {
	/** `library`, `project` or `repo`. */
	layer?: string;
	repo?: string;
	sha?: string;
	path?: string;
}

export interface GraphNode {
	id: string;
	kind: string;
	label?: string;
}

export interface GraphEdge {
	from: string;
	to: string;
	label?: string;
}

export interface PhaseView {
	id: string;
	use?: string;
	origin?: Origin;
	params?: Record<string, unknown>;
	with?: Record<string, unknown>;
	exits?: Record<string, unknown>;
	graph?: { nodes: GraphNode[]; edges: GraphEdge[] };
}

export interface WorkflowView {
	ref: string;
	origin?: Origin;
	phases: PhaseView[];
}

export interface BindingView {
	id: string;
	on?: unknown;
	if?: string;
	use: string;
	disabled?: boolean;
	origin?: Origin;
}

export interface LintMessage {
	level: string;
	message: string;
	path?: string;
}

export interface WorkflowViewDoc {
	bindings: BindingView[];
	workflows: WorkflowView[];
	lint?: LintMessage[];
}

export interface MetricsRow {
	workflow: string | null;
	phase: string;
	step: string;
	runs: number;
	avgWallSecs: number | null;
	activeSecs: number | null;
	queueSecs: number | null;
	inputTokens: number | null;
	outputTokens: number | null;
	costUsd: number | null;
	retries: number;
	resumes: number;
	stalls: number;
	steers: number;
}

export interface SecretName {
	name: string;
	updatedAt: string;
}

export interface ChatRow {
	runId: string;
	project: string;
	title: string | null;
	startedAt: string;
	lastAt: string | null;
	events: number;
}

export const SUBMIT_KINDS = ['command', 'comment', 'cancel', 'resume', 'stop', 'steer'] as const;
export type SubmitKind = (typeof SUBMIT_KINDS)[number];

const Id = v.pipe(v.number(), v.integer(), v.minValue(1));
const Body = v.pipe(v.string(), v.trim(), v.minLength(1), v.maxLength(65_536));
const Word = v.pipe(v.string(), v.regex(/^[A-Za-z0-9_-]{1,64}$/));

/** What the browser may post; the engine decides whether it applies. */
export const SubmitSchema = v.variant('kind', [
	v.object({
		id: Id,
		kind: v.literal('command'),
		data: v.object({
			command: Word,
			args: v.optional(v.pipe(v.string(), v.maxLength(4096)))
		})
	}),
	v.object({ id: Id, kind: v.picklist(['comment', 'steer']), data: v.object({ body: Body }) }),
	v.object({ id: Id, kind: v.picklist(['cancel', 'resume', 'stop']) })
]);
export type SubmitInput = v.InferInput<typeof SubmitSchema>;

export const SECRET_NAME = /^[A-Z_][A-Z0-9_]{0,63}$/;

/** The task page's comment box posts a steer while a step runs, a comment otherwise. */
export const commentKind = (status: string): 'steer' | 'comment' =>
	status === 'running' ? 'steer' : 'comment';

/** Cards per board column, in the order given; `column` comes from the projection. */
export function placeCards(
	board: Board,
	tasks: readonly TaskRow[]
): { column: BoardColumn; tasks: TaskRow[] }[] {
	const out = board.columns.map((column) => ({ column, tasks: [] as TaskRow[] }));
	const byId = new Map(out.map((c) => [c.column.id, c]));
	let other: { column: BoardColumn; tasks: TaskRow[] } | undefined;
	for (const t of tasks) {
		const slot = t.column ? byId.get(t.column) : undefined;
		if (slot) slot.tasks.push(t);
		else {
			other ??= { column: { id: '', title: 'Other' }, tasks: [] };
			other.tasks.push(t);
		}
	}
	return other ? [...out, other] : out;
}

/** "Waiting on you" rows grouped by project, projects in name order. */
export function groupByProject(tasks: readonly TaskRow[]): [string, TaskRow[]][] {
	const m = new Map<string, TaskRow[]>();
	for (const t of tasks) m.set(t.project, [...(m.get(t.project) ?? []), t]);
	return [...m.entries()].sort(([a], [b]) => a.localeCompare(b));
}

/** Replace, add or drop one changed row (`null` = it is gone or no longer visible). */
export function applyRow(
	rows: readonly TaskRow[],
	id: number,
	row: TaskRow | null,
	keep: (t: TaskRow) => boolean = () => true
): TaskRow[] {
	const rest = rows.filter((r) => r.id !== id);
	if (!row || !keep(row)) return rest;
	const i = rows.findIndex((r) => r.id === id);
	if (i < 0) return [row, ...rest];
	const copy = [...rows];
	copy[i] = row;
	return copy;
}

/** A GitHub source link for an origin, when it has a repo and sha. */
export function originUrl(o: Origin | undefined): string | null {
	if (!o?.repo || !o.sha) return null;
	const path = o.path ? `/blob/${o.sha}/${o.path}` : `/tree/${o.sha}`;
	return `https://github.com/${o.repo}${path}`;
}

/** Badge text for an origin: `library@abc1234`, `project`, `repo`. */
export function originLabel(o: Origin | undefined): string {
	if (!o) return 'unknown';
	const layer = o.layer ?? (o.repo ? o.repo : 'unknown');
	return o.sha ? `${layer}@${o.sha.slice(0, 7)}` : layer;
}

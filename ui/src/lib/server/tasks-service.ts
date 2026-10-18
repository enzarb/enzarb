import type { Sql } from './db.ts';
import type {
	Board,
	ChatRow,
	MetricsRow,
	SecretName,
	SessionEvent,
	SessionHit,
	StepRun,
	SubmitKind,
	TaskDetail,
	TaskRow,
	WorkflowViewDoc
} from '#lib/tasks.ts';

/**
 * Reads and writes the engine through `engine_api` only (contracts-v2). Every read is limited to
 * `projects` (the caller's own); the service decides nothing about workflow state.
 */

export class TaskError extends Error {
	constructor(
		readonly kind: 'not-found' | 'invalid',
		message: string
	) {
		super(message);
	}
}

const num = (x: unknown): number | null => {
	if (x === null || x === undefined || x === '') return null;
	const n = Number(x);
	return Number.isFinite(n) ? n : null;
};
const iso = (x: unknown): string | null =>
	x instanceof Date ? x.toISOString() : typeof x === 'string' ? x : null;
const arr = <T>(x: unknown): T[] => (Array.isArray(x) ? (x as T[]) : []);

type Raw = Record<string, unknown>;

export function mapTask(r: Raw): TaskRow {
	return {
		id: Number(r.task_id),
		project: String(r.project),
		repo: (r.repo as string | null) ?? null,
		workflowRef: (r.workflow_ref as string | null) ?? null,
		title: String(r.title ?? ''),
		subjects: arr(r.subjects),
		status: String(r.status),
		endStatus: (r.end_status as string | null) ?? null,
		phase: String(r.phase ?? ''),
		node: String(r.node ?? ''),
		column: (r.column as string | null) ?? null,
		waitingOn: (r.waiting_on as string | null) ?? null,
		reason: (r.reason as string | null) ?? null,
		text: (r.text as string | null) ?? null,
		actions: arr(r.actions),
		commands: arr(r.commands),
		artifacts: arr(r.artifacts),
		needsYou: r.needs_you === true,
		costUsd: num(r.cost_usd),
		createdAt: iso(r.created_at) ?? '',
		updatedAt: iso(r.updated_at) ?? '',
		endedAt: iso(r.ended_at)
	};
}

function mapStepRun(r: Raw): StepRun {
	return {
		runId: String(r.run_id),
		node: String(r.node ?? ''),
		step: String(r.step ?? ''),
		attempt: Number(r.attempt ?? 1),
		runtime: (r.runtime as string | null) ?? null,
		model: (r.model as string | null) ?? null,
		startedAt: iso(r.started_at),
		endedAt: iso(r.ended_at),
		activeSecs: num(r.active_secs),
		queueSecs: num(r.queue_secs),
		exit: (r.exit as string | null) ?? null,
		tokens: (r.tokens as Record<string, number> | null) ?? null,
		costUsd: num(r.cost_usd),
		resumes: Number(r.resumes ?? 0),
		stalls: Number(r.stalls ?? 0),
		steers: Number(r.steers ?? 0)
	};
}

export function mapDetail(d: Raw): TaskDetail {
	const base = mapTask(d);
	return {
		...base,
		artifacts: arr(d.artifacts),
		transitions: arr<Raw>(d.transitions).map((t) => ({
			at: iso(t.at) ?? '',
			eventType: (t.event_type as string | null) ?? null,
			from: (t.from as TaskDetail['transitions'][number]['from']) ?? { status: null, node: null },
			to: (t.to as TaskDetail['transitions'][number]['to']) ?? { status: null, node: null },
			effects: Number(t.effects ?? 0)
		})),
		stepRuns: arr<Raw>(d.step_runs).map(mapStepRun)
	};
}

export interface Actor {
	login: string;
	id: number;
}

export const SESSION_PAGE = 500;

export function tasksService(sql: Sql, projects: readonly string[]) {
	const owned = new Set(projects);
	const list = [...projects];

	async function detail(id: number): Promise<TaskDetail | null> {
		const [row] = await sql<{ d: Raw | null }[]>`SELECT engine_api.task_detail(${id}) AS d`;
		if (!row?.d || !owned.has(String(row.d.project))) return null;
		return mapDetail(row.d);
	}

	return {
		/** Tasks of one owned project, or of all of them. */
		async list(project?: string): Promise<TaskRow[]> {
			if (project !== undefined && !owned.has(project)) return [];
			const scope = project === undefined ? list : [project];
			if (scope.length === 0) return [];
			const rows = await sql<Raw[]>`
				SELECT * FROM engine_api.task_v WHERE project = ANY(${scope})
				ORDER BY updated_at DESC LIMIT 1000`;
			return rows.map(mapTask);
		},

		/** Rows with `needs_you` across the caller's projects, oldest first. */
		async needsYou(): Promise<TaskRow[]> {
			if (list.length === 0) return [];
			const rows = await sql<Raw[]>`
				SELECT * FROM engine_api.task_v WHERE project = ANY(${list}) AND needs_you
				ORDER BY updated_at ASC LIMIT 500`;
			return rows.map(mapTask);
		},

		/** One row, or null when missing or in another user's project. */
		async get(id: number): Promise<TaskRow | null> {
			const [r] = await sql<Raw[]>`SELECT * FROM engine_api.task_v WHERE task_id = ${id}`;
			return r && owned.has(String(r.project)) ? mapTask(r) : null;
		},

		detail,

		/** A page of a step run's session; the run must belong to an owned task. */
		async session(
			taskId: number,
			runId: string,
			afterSeq = 0,
			limit = SESSION_PAGE
		): Promise<SessionEvent[]> {
			const d = await detail(taskId);
			if (!d || !d.stepRuns.some((s) => s.runId === runId))
				throw new TaskError('not-found', 'Run not found');
			return readSession(sql, runId, afterSeq, limit);
		},

		async search(project: string, query: string): Promise<SessionHit[]> {
			if (!owned.has(project)) throw new TaskError('not-found', 'Project not found');
			const rows = await sql<Raw[]>`SELECT * FROM engine_api.session_search(${project}, ${query})`;
			return rows.map((r) => ({
				runId: String(r.run_id),
				taskId: Number(r.task_id),
				seq: Number(r.seq),
				at: iso(r.at) ?? '',
				kind: String(r.kind),
				snippet: String(r.snippet ?? '')
			}));
		},

		async board(project: string): Promise<Board> {
			if (!owned.has(project)) throw new TaskError('not-found', 'Project not found');
			const [r] = await sql<{ b: Board | null }[]>`SELECT engine_api.board_v(${project}) AS b`;
			return { columns: arr(r?.b?.columns) };
		},

		async workflow(project: string, repo: string): Promise<WorkflowViewDoc | null> {
			if (!owned.has(project)) throw new TaskError('not-found', 'Project not found');
			const [r] = await sql<
				{ w: WorkflowViewDoc | null }[]
			>`SELECT engine_api.workflow_v(${project}, ${repo}) AS w`;
			if (!r?.w) return null;
			return { bindings: arr(r.w.bindings), workflows: arr(r.w.workflows), lint: arr(r.w.lint) };
		},

		async metrics(project: string, from: Date, to: Date): Promise<MetricsRow[]> {
			if (!owned.has(project)) throw new TaskError('not-found', 'Project not found');
			const rows = await sql<Raw[]>`SELECT * FROM engine_api.metrics(${project}, ${from}, ${to})`;
			return rows.map((r) => ({
				workflow: (r.workflow as string | null) ?? null,
				phase: String(r.phase ?? ''),
				step: String(r.step ?? ''),
				runs: Number(r.runs ?? 0),
				avgWallSecs: num(r.avg_wall_secs),
				activeSecs: num(r.active_secs),
				queueSecs: num(r.queue_secs),
				inputTokens: num(r.input_tokens),
				outputTokens: num(r.output_tokens),
				costUsd: num(r.cost_usd),
				retries: Number(r.retries ?? 0),
				resumes: Number(r.resumes ?? 0),
				stalls: Number(r.stalls ?? 0),
				steers: Number(r.steers ?? 0)
			}));
		},

		/** Posts one UI event; the engine decides what it means. Returns the event id. */
		async submit(id: number, actor: Actor, kind: SubmitKind, data: unknown = {}): Promise<number> {
			const row = await this.get(id);
			if (!row) throw new TaskError('not-found', 'Task not found');
			const [r] = await sql<{ id: string }[]>`
				SELECT engine_api.submit(${id}, ${sql.json(actor as never)}, ${kind},
					${sql.json((data ?? {}) as never)}) AS id`;
			return Number(r.id);
		}
	};
}

export async function readSession(
	sql: Sql,
	runId: string,
	afterSeq = 0,
	limit = SESSION_PAGE
): Promise<SessionEvent[]> {
	const rows = await sql<Raw[]>`
		SELECT * FROM engine_api.session_v(${runId}, ${afterSeq}, ${Math.min(limit, SESSION_PAGE)})`;
	return rows.map((r) => ({
		seq: Number(r.seq),
		at: iso(r.at) ?? '',
		kind: String(r.kind),
		payload: r.payload,
		truncated: r.truncated === true
	}));
}

export type SecretScope = 'user' | 'project';

export function secretsService(sql: Sql) {
	return {
		async list(scope: SecretScope, owner: string): Promise<SecretName[]> {
			const rows = await sql<
				{ name: string; updated_at: Date | string }[]
			>`SELECT * FROM engine_api.secret_list(${scope}, ${owner})`;
			return rows.map((r) => ({ name: r.name, updatedAt: iso(r.updated_at) ?? '' }));
		},
		async set(scope: SecretScope, owner: string, name: string, value: string): Promise<void> {
			await sql`SELECT engine_api.secret_set(${scope}, ${owner}, ${name}, ${value})`;
		},
		async remove(scope: SecretScope, owner: string, name: string): Promise<boolean> {
			const [r] = await sql<
				{ ok: boolean }[]
			>`SELECT engine_api.secret_delete(${scope}, ${owner}, ${name}) AS ok`;
			return r?.ok ?? false;
		}
	};
}

export function chatsService(sql: Sql, user: string) {
	async function list(project?: string): Promise<ChatRow[]> {
		const rows = await sql<Raw[]>`SELECT * FROM engine_api.chat_v(${user})`;
		return rows
			.map((r) => ({
				runId: String(r.run_id),
				project: String(r.project),
				title: (r.title as string | null) ?? null,
				startedAt: iso(r.started_at) ?? '',
				lastAt: iso(r.last_at),
				events: Number(r.events ?? 0)
			}))
			.filter((c) => project === undefined || c.project === project);
	}
	return {
		list,
		/** A chat's session; only the user's own chats. */
		async session(runId: string, afterSeq = 0): Promise<SessionEvent[]> {
			if (!runId.startsWith('chat-') || !(await list()).some((c) => c.runId === runId))
				throw new TaskError('not-found', 'Chat not found');
			return readSession(sql, runId, afterSeq);
		}
	};
}

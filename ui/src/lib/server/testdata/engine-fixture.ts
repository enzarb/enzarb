import { readFileSync, readdirSync } from 'node:fs';
import type { Sql } from '../db.ts';

const migrationsDir = new URL('../../../../../workflows/migrations/', import.meta.url);

/**
 * Applies the real `engine` / `engine_api` schemas: workflows/migrations/*.sql in filename order,
 * after creating the runtime roles their grants target (pgop creates them in a cluster).
 */
export async function applyEngineSchema(sql: Sql): Promise<void> {
	for (const role of ['enzarb_ui', 'enzarb_workflows', 'enzarb_connectors', 'enzarb_realtime']) {
		await sql.unsafe(
			`DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${role}') THEN CREATE ROLE ${role}; END IF; END $$`
		);
	}
	const files = readdirSync(migrationsDir)
		.filter((f) => f.endsWith('.sql'))
		.sort();
	for (const f of files) await sql.unsafe(readFileSync(new URL(f, migrationsDir), 'utf8'));
}

export async function seedBoard(
	sql: Sql,
	project: string,
	repo: string,
	board: object
): Promise<void> {
	await sql`INSERT INTO engine.resolutions (project, repo, board) VALUES (${project}, ${repo},
		${sql.json(board as never)})
		ON CONFLICT (project, repo) DO UPDATE SET board = EXCLUDED.board`;
}

export async function seedWorkflowView(
	sql: Sql,
	project: string,
	repo: string,
	view: object
): Promise<void> {
	await sql`INSERT INTO engine.resolutions (project, repo, view) VALUES (${project}, ${repo},
		${sql.json(view as never)})
		ON CONFLICT (project, repo) DO UPDATE SET view = EXCLUDED.view`;
}

export interface SeedTask {
	project: string;
	title: string;
	status?: string;
	endStatus?: string | null;
	repo?: string;
	workflowRef?: string;
	phase?: string;
	node?: string;
	column?: string | null;
	waitingOn?: string | null;
	reason?: string | null;
	text?: string | null;
	needsYou?: boolean;
	actions?: { label: string; command: string }[];
	artifacts?: { title: string; format: string; content: string; node?: string }[];
	subjects?: { kind: string; key: string; url?: string }[];
	costUsd?: number | null;
}

export async function seedTask(sql: Sql, t: SeedTask): Promise<number> {
	const [row] = await sql<{ id: string }[]>`
		INSERT INTO engine.tasks (project, repo, workflow_ref, state, resolved)
		VALUES (${t.project}, ${t.repo ?? 'acme/web'}, ${t.workflowRef ?? 'std/workflows/issue'}, '{}', '{}')
		RETURNING id`;
	const id = Number(row.id);
	await sql`
		INSERT INTO engine.projection (task_id, project, repo, workflow, title, subjects, status, end_status, phase, node,
			"column", waiting_on, reason, text, actions, artifacts, needs_you, cost_usd)
		VALUES (${id}, ${t.project}, ${t.repo ?? 'acme/web'}, ${t.workflowRef ?? 'std/workflows/issue'}, ${t.title}, ${sql.json((t.subjects ?? []) as never)},
			${t.status ?? 'running'}, ${t.endStatus ?? null}, ${t.phase ?? 'plan'}, ${t.node ?? 'plan'},
			${t.column ?? null}, ${t.waitingOn ?? null}, ${t.reason ?? null}, ${t.text ?? null},
			${sql.json((t.actions ?? []) as never)}, ${sql.json((t.artifacts ?? []) as never)},
			${t.needsYou ?? false}, ${t.costUsd ?? 0})`;
	return id;
}

export async function seedStepRun(
	sql: Sql,
	taskId: number,
	runId: string,
	r: { node?: string; step?: string; ended?: boolean; cost?: number; tokens?: object } = {}
): Promise<void> {
	await sql`
		INSERT INTO engine.step_runs (run_id, task_id, project, node, step, runtime, model, started_at, ended_at,
			active_secs, queue_secs, exit, tokens, cost_usd)
		SELECT ${runId}, ${taskId}, t.project, ${r.node ?? 'plan/plan'}, ${r.step ?? 'plan'}, 'claude', 'opus',
			now() - interval '10 minutes', ${r.ended === false ? null : sql`now()`}, 300, 5,
			${r.ended === false ? null : 'done'}, ${sql.json((r.tokens ?? { input: 1000, output: 200 }) as never)},
			${r.cost ?? 0.5} FROM engine.tasks t WHERE t.id = ${taskId}`;
}

export async function seedSessionEvent(
	sql: Sql,
	runId: string,
	seq: number,
	kind: string,
	payload: unknown
): Promise<void> {
	await sql`INSERT INTO engine.session_events (run_id, seq, at, kind, payload)
		VALUES (${runId}, ${seq}, now(), ${kind}, ${sql.json(payload as never)})`;
}

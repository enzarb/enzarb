import { readFileSync } from 'node:fs';
import type { Sql } from '../db.ts';

/**
 * Applies the local `engine` / `engine_api` fixture (engine.sql).
 * TODO(M7 integration): apply the monorepo's real workflows/migrations instead, once the engine
 * core branch lands, and delete engine.sql.
 */
export async function applyEngineSchema(sql: Sql): Promise<void> {
	await sql.unsafe(readFileSync(new URL('./engine.sql', import.meta.url), 'utf8'));
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
		INSERT INTO engine.tasks (project, repo, workflow_ref)
		VALUES (${t.project}, ${t.repo ?? null}, ${t.workflowRef ?? 'std/workflows/issue'}) RETURNING id`;
	const id = Number(row.id);
	await sql`
		INSERT INTO engine.projection (task_id, project, title, subjects, status, end_status, phase, node,
			"column", waiting_on, reason, text, actions, artifacts, needs_you, cost_usd)
		VALUES (${id}, ${t.project}, ${t.title}, ${sql.json((t.subjects ?? []) as never)},
			${t.status ?? 'running'}, ${t.endStatus ?? null}, ${t.phase ?? 'plan'}, ${t.node ?? 'plan'},
			${t.column ?? null}, ${t.waitingOn ?? null}, ${t.reason ?? null}, ${t.text ?? null},
			${sql.json((t.actions ?? []) as never)}, ${sql.json((t.artifacts ?? []) as never)},
			${t.needsYou ?? false}, ${t.costUsd ?? null})`;
	return id;
}

export async function seedStepRun(
	sql: Sql,
	taskId: number,
	runId: string,
	r: { node?: string; step?: string; ended?: boolean; cost?: number; tokens?: object } = {}
): Promise<void> {
	await sql`
		INSERT INTO engine.step_runs (run_id, task_id, node, step, runtime, model, started_at, ended_at,
			active_secs, queue_secs, exit, tokens, cost_usd)
		VALUES (${runId}, ${taskId}, ${r.node ?? 'plan/plan'}, ${r.step ?? 'plan'}, 'claude', 'opus',
			now() - interval '10 minutes', ${r.ended === false ? null : sql`now()`}, 300, 5,
			${r.ended === false ? null : 'done'}, ${sql.json((r.tokens ?? { input: 1000, output: 200 }) as never)},
			${r.cost ?? 0.5})`;
}

export async function seedSessionEvent(
	sql: Sql,
	runId: string,
	seq: number,
	kind: string,
	payload: unknown
): Promise<void> {
	await sql`INSERT INTO engine.session_events (run_id, seq, kind, payload)
		VALUES (${runId}, ${seq}, ${kind}, ${sql.json(payload as never)})`;
}

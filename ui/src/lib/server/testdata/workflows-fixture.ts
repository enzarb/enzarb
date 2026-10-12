import { readdirSync, readFileSync } from 'node:fs';
import type { Sql } from '../db.ts';

const dir = new URL('./workflows/', import.meta.url);

/**
 * Applies the engine's real `workflows` migrations in filename order, so the tables and
 * `workflows_api` views/functions are their cumulative result.
 */
export async function applyWorkflowsSchema(sql: Sql): Promise<void> {
	const files = readdirSync(dir)
		.filter((f) => f.endsWith('.sql'))
		.sort();
	for (const f of files) await sql.unsafe(readFileSync(new URL(f, dir), 'utf8'));
}

export interface SeedItem {
	key: string;
	project: string | null;
	workflow?: string;
	state: string;
	stateData?: unknown;
}

export async function seedItem(sql: Sql, item: SeedItem): Promise<number> {
	const [row] = await sql<{ id: string }[]>`
		INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data)
		VALUES (${item.key}, ${item.project}, ${item.workflow ?? 'issue'}, ${item.state},
			${sql.json((item.stateData ?? {}) as never)})
		RETURNING id`;
	return Number(row.id);
}

export async function seedTransition(
	sql: Sql,
	workItemId: number,
	from: string | null,
	to: string,
	eventType: string
): Promise<void> {
	await sql`
		INSERT INTO workflows.transitions (work_item_id, from_state, to_state, event_type)
		VALUES (${workItemId}, ${from}, ${to}, ${eventType})`;
}

export async function seedJob(
	sql: Sql,
	workItemId: number,
	kind: string,
	status: string,
	output: unknown = null
): Promise<void> {
	await sql`
		INSERT INTO workflows.jobs (work_item_id, kind, status, idempotency_key, attempt, output)
		VALUES (${workItemId}, ${kind}, ${status}, ${`k:${workItemId}:${kind}:${Math.random()}`}, 1,
			${output === null ? null : sql.json(output as never)})`;
}

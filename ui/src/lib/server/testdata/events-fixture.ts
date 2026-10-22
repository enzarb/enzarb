import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import type { Sql } from '../db.ts';

/** The receiver's migrations (connectors/), the single source of the `connectors` schema. */
const MIGRATIONS = path.resolve(
	import.meta.dirname,
	'../../../../../connectors/bin/receiver/migrations'
);

/** Applies the receiver's migrations, so tests read the real `connectors.event_v`. */
export async function applyConnectorsSchema(sql: Sql): Promise<void> {
	const files = (await readdir(MIGRATIONS)).filter((f) => f.endsWith('.sql')).sort();
	for (const f of files) await sql.unsafe(await readFile(path.join(MIGRATIONS, f), 'utf8'));
}

export interface SeedEvent {
	repo: string | null;
	event: string;
	action?: string | null;
	sender?: string | null;
	payload?: unknown;
}

let n = 0;

/** Inserts one delivery the way the receiver does (without the NOTIFY). Returns its id. */
export async function seedEvent(sql: Sql, e: SeedEvent): Promise<number> {
	n += 1;
	const rows = await sql<{ id: string }[]>`
		INSERT INTO connectors.events (delivery_id, event, action, repo, sender, payload)
		VALUES (${`seed-${n}-${Date.now()}`}, ${e.event}, ${e.action ?? null}, ${e.repo},
			${e.sender ?? null}, ${sql.json((e.payload ?? {}) as never)})
		RETURNING id`;
	return Number(rows[0]!.id);
}

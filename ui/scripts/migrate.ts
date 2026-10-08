// Plain-SQL migration runner: `npm run migrate` (node strips the types).
// Applies migrations/*.sql in filename order inside one transaction each,
// tracked in app._migrations. Run with the schema owner's DATABASE_URL.
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import postgres from 'postgres';

const defaultDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../migrations');

export async function migrate(sql: postgres.Sql, dir = defaultDir): Promise<string[]> {
	const files = (await readdir(dir)).filter((f) => f.endsWith('.sql')).sort();
	await sql`CREATE SCHEMA IF NOT EXISTS app`;
	await sql`CREATE TABLE IF NOT EXISTS app._migrations (
		name text PRIMARY KEY,
		applied_at timestamptz NOT NULL DEFAULT now()
	)`;
	const applied: string[] = [];
	for (const name of files) {
		const body = await readFile(path.join(dir, name), 'utf8');
		const done = await sql.begin(async (tx) => {
			// Serialize concurrent runners (e.g. two pods) on the migration table.
			await tx`LOCK TABLE app._migrations IN EXCLUSIVE MODE`;
			const [row] = await tx`SELECT 1 AS one FROM app._migrations WHERE name = ${name}`;
			if (row) return false;
			await tx.unsafe(body);
			await tx`INSERT INTO app._migrations (name) VALUES (${name})`;
			return true;
		});
		if (done) applied.push(name);
	}
	return applied;
}

if (import.meta.main) {
	const url = process.env.DATABASE_URL;
	if (!url) {
		console.error('DATABASE_URL is required');
		process.exit(1);
	}
	const sql = postgres(url, { max: 1, onnotice: () => {} });
	try {
		const applied = await migrate(sql);
		console.log(applied.length ? `applied: ${applied.join(', ')}` : 'up to date');
	} finally {
		await sql.end();
	}
}

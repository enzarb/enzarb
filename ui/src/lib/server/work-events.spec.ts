import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { WorkEventHub, type WorkItemChange } from './work-events.ts';
import { applyWorkflowsSchema, seedItem } from './testdata/workflows-fixture.ts';

describe('WorkEventHub', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;
	let id: number;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await applyWorkflowsSchema(sql);
		id = await seedItem(sql, { key: 'github:acme/web#1', project: 'web', state: 'Triaging' });
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	it('fans one work_item_changed notification out with the project name', async () => {
		const hub = new WorkEventHub(sql);
		const a: WorkItemChange[] = [];
		const b: WorkItemChange[] = [];
		const offA = hub.subscribe((c) => a.push(c));
		const offB = hub.subscribe((c) => b.push(c));
		// Wait for LISTEN to be established, then notify like the reducer does.
		await expect
			.poll(async () => {
				await sql`SELECT pg_notify('work_item_changed', ${String(id)})`;
				return a.length;
			})
			.toBeGreaterThan(0);
		expect(a[0]).toEqual({ id, project: 'web' });
		expect(b[0]).toEqual({ id, project: 'web' });
		offA();
		offB();
		expect(hub.size).toBe(0);
	});

	it('ignores malformed payloads', async () => {
		const hub = new WorkEventHub(sql);
		const got: WorkItemChange[] = [];
		const off = hub.subscribe((c) => got.push(c));
		await expect
			.poll(async () => {
				await sql`SELECT pg_notify('work_item_changed', 'not-a-number')`;
				await sql`SELECT pg_notify('work_item_changed', ${String(id)})`;
				return got.length;
			})
			.toBeGreaterThan(0);
		expect(got.every((c) => c.id === id)).toBe(true);
		off();
	});
});

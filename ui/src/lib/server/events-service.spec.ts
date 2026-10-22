import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import type { Sql } from './db.ts';
import { eventsService, projectRepoKeys } from './events-service.ts';
import { applyConnectorsSchema, seedEvent } from './testdata/events-fixture.ts';

describe('projectRepoKeys', () => {
	it('keys the repositories, skipping unparsable urls', () => {
		expect(
			projectRepoKeys({
				repositories: [
					{ url: 'https://github.com/Octo/App.git', connector: 'github', defaultBranch: 'main' },
					{ url: 'junk', connector: 'github', defaultBranch: 'main' }
				]
			})
		).toEqual(['octo/app']);
	});
});

describe('eventsService (connectors.event_v)', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { max: 2, onnotice: () => {} });
		await applyConnectorsSchema(sql);
	}, 120_000);
	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	const svc = () => eventsService(sql as unknown as Sql);

	it('returns only the given repositories, newest first, with a summary', async () => {
		const a = await seedEvent(sql as unknown as Sql, {
			repo: 'octo/app',
			event: 'issues',
			action: 'opened',
			payload: { issue: { title: 'First' } }
		});
		await seedEvent(sql as unknown as Sql, { repo: 'octo/other', event: 'push' });
		const b = await seedEvent(sql as unknown as Sql, { repo: 'octo/app', event: 'push' });
		const rows = await svc().list(['octo/app']);
		expect(rows.map((r) => r.id)).toEqual([b, a]);
		expect(rows[1]).toMatchObject({ event: 'issues', action: 'opened', summary: 'First' });
		expect(await svc().list([])).toEqual([]);
	});

	it('pages with before', async () => {
		const rows = await svc().list(['octo/app']);
		const next = await svc().list(['octo/app'], rows[0]!.id);
		expect(next.map((r) => r.id)).toEqual(rows.slice(1).map((r) => r.id));
	});

	it('serves a payload only for the given repositories', async () => {
		const id = await seedEvent(sql as unknown as Sql, {
			repo: 'octo/app',
			event: 'push',
			payload: { ref: 'refs/heads/main' }
		});
		expect(await svc().payload(['octo/app'], id)).toEqual({ ref: 'refs/heads/main' });
		expect(await svc().payload(['octo/other'], id)).toBeNull();
		expect(await svc().payload([], id)).toBeNull();
	});

	it('returns events with payloads by id, only for the given repositories', async () => {
		const a = await seedEvent(sql as unknown as Sql, {
			repo: 'octo/app',
			event: 'issues',
			action: 'labeled',
			payload: { label: { name: 'ez' } }
		});
		const other = await seedEvent(sql as unknown as Sql, { repo: 'octo/other', event: 'push' });
		const rows = await svc().withPayloads(['octo/app'], [a, other, 999_999]);
		expect(rows).toEqual([
			{ id: a, event: 'issues', action: 'labeled', payload: { label: { name: 'ez' } } }
		]);
		expect(await svc().withPayloads(['octo/app'], [])).toEqual([]);
	});
});

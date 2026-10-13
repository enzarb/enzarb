import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({ sql: undefined as unknown, calls: 0 }));
vi.mock('#lib/server/db.ts', () => ({
	getSql: () => {
		state.calls++;
		return state.sql;
	}
}));

import { isDatabaseReady } from '#lib/server/health.ts';
import { GET as healthz } from './healthz/+server.ts';
import { GET as readyz } from './readyz/+server.ts';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const call = (h: any) => h({} as never) as Response | Promise<Response>;

beforeEach(() => {
	state.calls = 0;
});

describe('/healthz', () => {
	it('returns 200 ok without touching the database', async () => {
		const res = await call(healthz);
		expect(res.status).toBe(200);
		expect(await res.text()).toBe('ok');
		expect(state.calls).toBe(0);
	});
});

describe('/readyz with postgres', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	it('returns 200 when the database answers', async () => {
		state.sql = sql;
		expect(await isDatabaseReady(sql)).toBe(true);
		expect((await call(readyz)).status).toBe(200);
	});

	it('returns 503 when the database is down', async () => {
		const down = postgres(container.getConnectionUri(), {
			onnotice: () => {},
			connect_timeout: 1
		});
		await container.stop();
		state.sql = down;
		const res = await call(readyz);
		expect(res.status).toBe(503);
		await down.end({ timeout: 0 });
	}, 30_000);
});

describe('isDatabaseReady', () => {
	it('returns false when the query hangs past the timeout', async () => {
		const hang = (() => new Promise(() => {})) as unknown as postgres.Sql;
		expect(await isDatabaseReady(hang, 50)).toBe(false);
	});
});

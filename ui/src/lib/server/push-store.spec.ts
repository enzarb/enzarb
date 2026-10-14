import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { migrate } from '../../../scripts/migrate.ts';
import type { Sql } from './db.ts';
import { pushStore } from './push-store.ts';

const sub = (
	endpoint: string,
	over: Partial<Parameters<ReturnType<typeof pushStore>['upsert']>[1]> = {}
) => ({
	endpoint,
	p256dh: 'key',
	auth: 'auth',
	categories: ['approval', 'merge'] as ('approval' | 'merge' | 'blocked' | 'failed')[],
	userAgent: 'UA',
	...over
});

describe('push subscriptions and sent-log (app_api.push_*)', () => {
	let container: StartedPostgreSqlContainer;
	let admin: postgres.Sql;
	let owner: postgres.Sql;

	// The UI runs as enzarb_ui: do everything through that role so the grants are exercised too.
	const asUi = async <T>(fn: (store: ReturnType<typeof pushStore>) => Promise<T>): Promise<T> =>
		(await admin.begin(async (tx) => {
			await tx.unsafe('SET LOCAL ROLE enzarb_ui');
			const r = await fn(pushStore(tx as unknown as Sql));
			await tx.unsafe('RESET ROLE');
			return r;
		})) as T;
	const store = {
		upsert: (o: string, s: ReturnType<typeof sub>) => asUi((x) => x.upsert(o, s)),
		list: (o: string) => asUi((x) => x.list(o)),
		targets: (o: string, c: Parameters<ReturnType<typeof pushStore>['targets']>[1]) =>
			asUi((x) => x.targets(o, c)),
		setCategories: (
			o: string,
			e: string,
			c: Parameters<ReturnType<typeof pushStore>['setCategories']>[2]
		) => asUi((x) => x.setCategories(o, e, c)),
		remove: (o: string, e: string) => asUi((x) => x.remove(o, e)),
		claim: (o: string, id: number, c: 'approval' | 'merge', k: string) =>
			asUi((x) => x.claim(o, id, c, k, 60)),
		prune: (d: number) => asUi((x) => x.prune(d))
	};

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		const root = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await root.unsafe(`CREATE ROLE ps_owner LOGIN PASSWORD 'x'`);
		await root.unsafe(`CREATE ROLE enzarb_ui`);
		await root.unsafe(`CREATE DATABASE enzarb OWNER ps_owner`);
		await root.end();
		const u = new URL(container.getConnectionUri());
		u.pathname = '/enzarb';
		const asOwner = new URL(u);
		asOwner.username = 'ps_owner';
		asOwner.password = 'x';
		owner = postgres(asOwner.toString(), { max: 1, onnotice: () => {} });
		await migrate(owner);
		admin = postgres(u.toString(), { onnotice: () => {} });
	}, 120_000);

	afterAll(async () => {
		await owner?.end();
		await admin?.end();
		await container?.stop();
	});

	it('gives the UI role no direct access to the tables', async () => {
		for (const table of ['app.push_subscriptions', 'app.push_sent']) {
			for (const priv of ['SELECT', 'INSERT', 'UPDATE', 'DELETE']) {
				const [r] = await admin`SELECT has_table_privilege('enzarb_ui', ${table}, ${priv}) AS ok`;
				expect(r.ok, `${table} ${priv}`).toBe(false);
			}
		}
	});

	it('upserts one row per endpoint and lists only the owner devices, without keys', async () => {
		expect(await store.upsert('alice', sub('https://p/a1'))).toBe(true);
		expect(await store.upsert('alice', sub('https://p/a1', { categories: ['failed'] }))).toBe(true);
		expect(await store.upsert('bob', sub('https://p/b1'))).toBe(true);
		const alice = await store.list('alice');
		expect(alice.map((d) => d.endpoint)).toEqual(['https://p/a1']);
		expect(alice[0].categories).toEqual(['failed']);
		expect(Object.keys(alice[0])).not.toContain('p256dh');
		expect((await store.list('bob')).map((d) => d.endpoint)).toEqual(['https://p/b1']);
		expect(await store.list('nobody')).toEqual([]);
	});

	it('rejects taking over another owner endpoint and leaves it untouched', async () => {
		expect(await store.upsert('mallory', sub('https://p/a1', { p256dh: 'evil' }))).toBe(false);
		const targets = await store.targets('alice', null);
		expect(targets).toEqual([{ endpoint: 'https://p/a1', p256dh: 'key', auth: 'auth' }]);
		expect(await store.list('mallory')).toEqual([]);
	});

	it('filters targets by category and owner', async () => {
		expect((await store.targets('bob', 'merge')).map((t) => t.endpoint)).toEqual(['https://p/b1']);
		expect(await store.targets('bob', 'failed')).toEqual([]);
		expect(await store.targets('alice', 'merge')).toEqual([]);
	});

	it('only the owner can change or delete a device', async () => {
		expect(await store.setCategories('mallory', 'https://p/b1', [])).toBe(false);
		expect(await store.remove('mallory', 'https://p/b1')).toBe(false);
		expect(await store.setCategories('bob', 'https://p/b1', ['blocked'])).toBe(true);
		expect((await store.list('bob'))[0].categories).toEqual(['blocked']);
		expect(await store.remove('bob', 'https://p/b1')).toBe(true);
		expect(await store.list('bob')).toEqual([]);
	});

	it('claims a transition once, counts recent claims and prunes old rows', async () => {
		expect(await store.claim('alice', 7, 'approval', 'AwaitingApproval:1')).toEqual({
			claimed: true,
			recent: 1
		});
		expect(await store.claim('alice', 7, 'approval', 'AwaitingApproval:1')).toEqual({
			claimed: false,
			recent: 1
		});
		expect((await store.claim('alice', 7, 'approval', 'AwaitingApproval:2')).claimed).toBe(true);
		expect((await store.claim('alice', 7, 'merge', 'AwaitingApproval:1')).claimed).toBe(true);
		// another owner's claims do not count
		expect(await store.claim('bob', 7, 'approval', 'AwaitingApproval:1')).toEqual({
			claimed: true,
			recent: 1
		});
		await owner`UPDATE app.push_sent SET sent_at = now() - interval '40 days' WHERE owner = 'bob'`;
		expect(await store.prune(30)).toBe(1);
		expect(await store.prune(30)).toBe(0);
	});
});

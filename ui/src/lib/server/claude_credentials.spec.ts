import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { migrate } from '../../../scripts/migrate.ts';

const FUNCTIONS = {
	put: 'app_api.claude_credential_put(text, text, bytea, boolean)',
	delete: 'app_api.claude_credential_delete(text)',
	get: 'app_api.claude_credential_get(text)',
	list_stale: 'app_api.claude_credential_list_stale(text)',
	rekey: 'app_api.claude_credential_rekey(text, text, text, bytea)'
};
const DELETE_OWN = 'app_api.claude_credential_delete_own(text)';
const ALL = Object.values(FUNCTIONS);

describe('app.claude_credentials migration', () => {
	let container: StartedPostgreSqlContainer;
	let admin: postgres.Sql; // superuser, connected to the main database
	let owner: postgres.Sql; // schema owner that runs migrations
	let plain: postgres.Sql;

	function urlFor(db: string, user?: string) {
		const u = new URL(container.getConnectionUri());
		u.pathname = `/${db}`;
		if (user) {
			u.username = user;
			u.password = 'x';
		}
		return u.toString();
	}

	// Run `fn` as `role` inside a transaction (SET LOCAL ROLE), rolled back afterwards unless it returns.
	async function as<T>(role: string, fn: (tx: postgres.TransactionSql) => Promise<T>): Promise<T> {
		return (await admin.begin(async (tx) => {
			await tx.unsafe(`SET LOCAL ROLE ${role}`);
			return fn(tx);
		})) as T;
	}

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		const root = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await root.unsafe(`CREATE ROLE cc_owner LOGIN PASSWORD 'x'`);
		// Database without any of the runtime roles: the guards must let the migration through.
		// Roles are cluster-wide, so this one is migrated before they are created.
		await root.unsafe(`CREATE DATABASE plain OWNER cc_owner`);
		plain = postgres(urlFor('plain', 'cc_owner'), { max: 1, onnotice: () => {} });
		await migrate(plain);

		await root.unsafe(`CREATE ROLE enzarb_ui`);
		await root.unsafe(`CREATE ROLE enzarb_sandbox_proxy`);
		await root.unsafe(`CREATE ROLE somebody_else`);
		await root.unsafe(`CREATE DATABASE enzarb OWNER cc_owner`);
		await root.end();
		owner = postgres(urlFor('enzarb', 'cc_owner'), { max: 1, onnotice: () => {} });
		await migrate(owner);
		admin = postgres(urlFor('enzarb'), { onnotice: () => {} });
	}, 120_000);

	afterAll(async () => {
		await plain?.end();
		await owner?.end();
		await admin?.end();
		await container?.stop();
	});

	it('applies without the runtime roles', async () => {
		const [row] = await plain`SELECT to_regclass('app_api.claude_credential_v') AS v`;
		expect(row.v).toBe('app_api.claude_credential_v');
	});

	describe('grants', () => {
		it('gives the UI role no access to the table', async () => {
			for (const priv of ['SELECT', 'INSERT', 'UPDATE', 'DELETE']) {
				const [r] = await admin`
					SELECT has_table_privilege('enzarb_ui', 'app.claude_credentials', ${priv}) AS ok`;
				expect(r.ok, priv).toBe(false);
			}
		});

		it('lets the UI role read the view and execute nothing', async () => {
			const [v] = await admin`
				SELECT has_table_privilege('enzarb_ui', 'app_api.claude_credential_v', 'SELECT') AS ok`;
			expect(v.ok).toBe(true);
			for (const fn of ALL) {
				const [r] = await admin`SELECT has_function_privilege('enzarb_ui', ${fn}, 'EXECUTE') AS ok`;
				expect(r.ok, fn).toBe(false);
			}
		});

		it('has EXECUTE for the UI role on delete_own only', async () => {
			const rows = await admin`
				SELECT p.oid::regprocedure::text AS fn, has_function_privilege('enzarb_ui', p.oid, 'EXECUTE') AS ok
				FROM pg_proc p
				WHERE p.pronamespace = 'app_api'::regnamespace AND p.proname LIKE 'claude_credential\_%'`;
			expect(rows).toHaveLength(6);
			for (const r of rows) expect(r.ok, r.fn).toBe(r.fn.includes('delete_own'));
		});

		it('re-asserts grants when roles are created after the migration', async () => {
			const root = postgres(container.getConnectionUri(), { onnotice: () => {} });
			try {
				await root.unsafe(`CREATE DATABASE late OWNER cc_owner`);
				const late = postgres(urlFor('late', 'cc_owner'), { max: 1, onnotice: () => {} });
				try {
					await migrate(late);
					// Roles already exist cluster-wide; revoke to simulate "missing at migration time".
					await root.end();
					const lateAdmin = postgres(urlFor('late'), { onnotice: () => {} });
					try {
						await lateAdmin.unsafe(
							`REVOKE ALL ON FUNCTION ${FUNCTIONS.get} FROM enzarb_sandbox_proxy`
						);
						await migrate(late);
						const [r] = await lateAdmin`
							SELECT has_function_privilege('enzarb_sandbox_proxy', ${FUNCTIONS.get}, 'EXECUTE') AS ok`;
						expect(r.ok).toBe(true);
					} finally {
						await lateAdmin.end();
					}
				} finally {
					await late.end();
				}
			} finally {
				await root.end().catch(() => {});
			}
		});

		it('lets the proxy role execute all five, not delete_own', async () => {
			const [d] = await admin`
				SELECT has_function_privilege('enzarb_sandbox_proxy', ${DELETE_OWN}, 'EXECUTE') AS ok`;
			expect(d.ok).toBe(false);
			for (const fn of ALL) {
				const [r] = await admin`
					SELECT has_function_privilege('enzarb_sandbox_proxy', ${fn}, 'EXECUTE') AS ok`;
				expect(r.ok, fn).toBe(true);
			}
		});

		it('does not leave EXECUTE with PUBLIC', async () => {
			for (const fn of [...ALL, DELETE_OWN]) {
				const [r] = await admin`
					SELECT has_function_privilege('somebody_else', ${fn}, 'EXECUTE') AS ok`;
				expect(r.ok, fn).toBe(false);
			}
		});

		it('makes every function SECURITY DEFINER with a pinned search_path', async () => {
			const rows = await admin`
				SELECT proname, prosecdef, proconfig FROM pg_proc
				WHERE pronamespace = 'app_api'::regnamespace AND proname LIKE 'claude_credential\_%'
					AND prokind = 'f'`;
			expect(rows).toHaveLength(6);
			for (const r of rows) {
				expect(r.prosecdef, r.proname).toBe(true);
				const expected =
					r.proname === 'claude_credential_delete_own'
						? 'search_path=pg_catalog, pg_temp'
						: 'search_path=pg_catalog, app';
				expect(r.proconfig, r.proname).toContain(expected);
			}
		});

		it('denies the UI role direct reads of the table', async () => {
			await expect(
				as('enzarb_ui', (tx) => tx`SELECT * FROM app.claude_credentials`)
			).rejects.toThrow(/permission denied/);
		});
	});

	describe('behaviour as the proxy role', () => {
		const bytes = (s: string) => Buffer.from(s);

		const put = (o: string, k: string, c: string, onlyIfAbsent: boolean) =>
			as('enzarb_sandbox_proxy', async (tx) => {
				const [r] = await tx`
					SELECT app_api.claude_credential_put(${o}, ${k}, ${bytes(c)}, ${onlyIfAbsent}) AS ok`;
				return r.ok as boolean;
			});
		const get = (o: string) =>
			as('enzarb_sandbox_proxy', (tx) => tx`SELECT * FROM app_api.claude_credential_get(${o})`);
		const rekey = (o: string, oldK: string, newK: string, c: string) =>
			as('enzarb_sandbox_proxy', async (tx) => {
				const [r] = await tx`
					SELECT app_api.claude_credential_rekey(${o}, ${oldK}, ${newK}, ${bytes(c)}) AS ok`;
				return r.ok as boolean;
			});

		it('puts and gets a credential', async () => {
			expect(await put('alice', 'k1', 'secret-1', false)).toBe(true);
			const rows = await get('alice');
			expect(rows).toHaveLength(1);
			expect(rows[0].key_id).toBe('k1');
			expect(Buffer.from(rows[0].ciphertext).toString()).toBe('secret-1');
		});

		it('does not overwrite an existing row when only_if_absent is true', async () => {
			expect(await put('alice', 'k2', 'secret-2', true)).toBe(true);
			const rows = await get('alice');
			expect(rows[0].key_id).toBe('k1');
			expect(Buffer.from(rows[0].ciphertext).toString()).toBe('secret-1');
		});

		it('inserts a new owner when only_if_absent is true', async () => {
			expect(await put('bob', 'k1', 'bob-secret', true)).toBe(true);
			const rows = await get('bob');
			expect(Buffer.from(rows[0].ciphertext).toString()).toBe('bob-secret');
		});

		it('replaces the row and bumps updated_at when only_if_absent is false', async () => {
			const [before] = await admin`
				SELECT updated_at FROM app.claude_credentials WHERE owner = 'alice'`;
			expect(await put('alice', 'k3', 'secret-3', false)).toBe(true);
			const rows = await get('alice');
			expect(rows[0].key_id).toBe('k3');
			expect(Buffer.from(rows[0].ciphertext).toString()).toBe('secret-3');
			const [after] = await admin`
				SELECT updated_at FROM app.claude_credentials WHERE owner = 'alice'`;
			expect(after.updated_at.getTime()).toBeGreaterThan(before.updated_at.getTime());
		});

		it('exposes only owner, present and updated_at through the view', async () => {
			const rows = await as(
				'enzarb_ui',
				(tx) => tx`SELECT * FROM app_api.claude_credential_v ORDER BY owner`
			);
			expect(rows.map((r) => r.owner)).toEqual(['alice', 'bob']);
			expect(Object.keys(rows[0]).sort()).toEqual(['owner', 'present', 'updated_at']);
			expect(rows[0].present).toBe(true);
		});

		it('lists rows under other key ids as stale', async () => {
			// alice is on k3, bob on k1
			const rows = await as(
				'enzarb_sandbox_proxy',
				(tx) => tx`SELECT * FROM app_api.claude_credential_list_stale('k3')`
			);
			expect(rows.map((r) => [r.owner, r.key_id])).toEqual([['bob', 'k1']]);
		});

		it('rekeys a row that still has the old key id', async () => {
			expect(await rekey('bob', 'k1', 'k3', 'bob-new')).toBe(true);
			const rows = await get('bob');
			expect(rows[0].key_id).toBe('k3');
			expect(Buffer.from(rows[0].ciphertext).toString()).toBe('bob-new');
			const stale = await as(
				'enzarb_sandbox_proxy',
				(tx) => tx`SELECT * FROM app_api.claude_credential_list_stale('k3')`
			);
			expect(stale).toHaveLength(0);
		});

		it('does nothing when rekey is given a stale old key id', async () => {
			expect(await rekey('bob', 'k1', 'k9', 'clobbered')).toBe(false);
			const rows = await get('bob');
			expect(rows[0].key_id).toBe('k3');
			expect(Buffer.from(rows[0].ciphertext).toString()).toBe('bob-new');
		});

		it('deletes a credential', async () => {
			await as(
				'enzarb_sandbox_proxy',
				(tx) => tx`SELECT app_api.claude_credential_delete('alice')`
			);
			expect(await get('alice')).toHaveLength(0);
			expect(await get('bob')).toHaveLength(1);
		});

		it('lets the UI role delete only the given owner row, returning a boolean', async () => {
			await put('a', 'k1', 'a-secret', false);
			await put('b', 'k1', 'b-secret', false);
			const del = (o: string) =>
				as('enzarb_ui', async (tx) => {
					const [r] = await tx`SELECT app_api.claude_credential_delete_own(${o}) AS ok`;
					return r.ok as boolean;
				});
			expect(await del('a')).toBe(true);
			expect(await get('a')).toHaveLength(0);
			expect(await get('b')).toHaveLength(1);
			expect(await del('a')).toBe(false);
			expect(await get('b')).toHaveLength(1);
		});
	});
});

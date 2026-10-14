import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { migrate } from '../../../scripts/migrate.ts';

// Regression test for enzarb-ui#26. On a pgop-provisioned database the `app` schema already exists,
// the migration role owns it, and a different role owns the database. `CREATE SCHEMA IF NOT EXISTS`
// then fails with "permission denied for database"; migrations must succeed anyway.
describe('migrations as a schema owner that does not own the database', () => {
	let container: StartedPostgreSqlContainer;
	let admin: postgres.Sql;
	let owner: postgres.Sql;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		admin = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await admin.unsafe(`CREATE ROLE wf_owner LOGIN PASSWORD 'x'`);
		await admin.unsafe(`CREATE ROLE ui_owner LOGIN PASSWORD 'x'`);
		await admin.unsafe(`CREATE ROLE enzarb_ui`); // runtime role granted access by migration 0001
		await admin.unsafe(`CREATE DATABASE enzarb OWNER wf_owner`);
		const inDb = new URL(container.getConnectionUri());
		inDb.pathname = '/enzarb';
		const setup = postgres(inDb.toString(), { onnotice: () => {} });
		await setup.unsafe(`CREATE SCHEMA app AUTHORIZATION ui_owner`);
		await setup.unsafe(`CREATE SCHEMA app_api AUTHORIZATION ui_owner`); // as pgop provisions it
		await setup.end();
		const asOwner = new URL(inDb);
		asOwner.username = 'ui_owner';
		asOwner.password = 'x';
		owner = postgres(asOwner.toString(), { max: 1, onnotice: () => {} });
	}, 120_000);

	afterAll(async () => {
		await owner?.end();
		await admin?.end();
		await container?.stop();
	});

	it('applies every migration, then is a no-op', async () => {
		expect(await migrate(owner)).toEqual([
			'0001_users_sessions.sql',
			'0002_project_settings.sql',
			'0003_user_repositories.sql',
			'0004_claude_credentials.sql',
			'0005_claude_credential_delete_own.sql',
			'0006_push_subscriptions.sql'
		]);
		expect(await migrate(owner)).toEqual([]);
		// The runtime role's grants (also made by the schema owner) took effect.
		const [row] =
			await owner`SELECT has_table_privilege('enzarb_ui', 'app.sessions', 'INSERT') AS ok`;
		expect(row.ok).toBe(true);
		const [repos] =
			await owner`SELECT has_table_privilege('enzarb_ui', 'app.user_repositories', 'INSERT') AS ok`;
		expect(repos.ok).toBe(true);
		// ...except on the credentials table, which the runtime role must not reach directly.
		const [creds] =
			await owner`SELECT has_table_privilege('enzarb_ui', 'app.claude_credentials', 'INSERT') AS ok`;
		expect(creds.ok).toBe(false);
	});
});

import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { projectSettings } from './project-settings.ts';
import { migrate } from '../../../scripts/migrate.ts';

import {
	SESSION_TTL_MS,
	createSession,
	generateSessionToken,
	hashToken,
	invalidateSession,
	upsertUser,
	validateSessionToken
} from './auth.ts';

describe('session tokens', () => {
	it('generates distinct url-safe tokens', () => {
		const a = generateSessionToken();
		expect(a).toMatch(/^[A-Za-z0-9_-]{27}$/);
		expect(generateSessionToken()).not.toBe(a);
	});

	it('hashes deterministically to sha-256 hex and never returns the token', () => {
		expect(hashToken('abc')).toBe(
			'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
		);
		expect(hashToken('abc')).toBe(hashToken('abc'));
		expect(hashToken('abd')).not.toBe(hashToken('abc'));
	});
});

describe('sessions in postgres', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;
	let userId: number;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await migrate(sql);
		userId = await upsertUser(sql, { id: 42, login: 'ruckc', name: 'Curtis', avatarUrl: null });
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	it('migrations are idempotent and tracked in app._migrations', async () => {
		expect(await migrate(sql)).toEqual([]);
		const rows = await sql`SELECT name FROM app._migrations`;
		expect(rows.map((r) => r.name)).toEqual([
			'0001_users_sessions.sql',
			'0002_project_settings.sql',
			'0003_user_repositories.sql'
		]);
	});

	it('upsertUser updates the profile of an existing GitHub id', async () => {
		const again = await upsertUser(sql, { id: 42, login: 'renamed', name: null, avatarUrl: 'x' });
		expect(again).toBe(userId);
		const [u] = await sql`SELECT login FROM app.users WHERE id = ${userId}`;
		expect(u.login).toBe('renamed');
	});

	it('stores only the hash and validates the token', async () => {
		const { token, session } = await createSession(sql, userId);
		const [row] = await sql`SELECT id FROM app.sessions WHERE id = ${session.id}`;
		expect(row.id).toBe(hashToken(token));
		expect(row.id).not.toBe(token);

		const ok = await validateSessionToken(sql, token);
		expect(ok?.user.githubId).toBe(42);
		expect(await validateSessionToken(sql, 'bogus')).toBeNull();
	});

	it('rejects and deletes expired sessions', async () => {
		const { token, session } = await createSession(
			sql,
			userId,
			new Date(Date.now() - SESSION_TTL_MS - 1000)
		);
		expect(await validateSessionToken(sql, token)).toBeNull();
		expect(await sql`SELECT 1 FROM app.sessions WHERE id = ${session.id}`).toHaveLength(0);
	});

	it('extends sessions in the second half of their lifetime', async () => {
		const now = new Date();
		const { token, session } = await createSession(
			sql,
			userId,
			new Date(now.getTime() - SESSION_TTL_MS * 0.6)
		);
		const result = await validateSessionToken(sql, token, now);
		expect(result?.session.expiresAt.getTime()).toBe(now.getTime() + SESSION_TTL_MS);
		const [row] = await sql`SELECT expires_at FROM app.sessions WHERE id = ${session.id}`;
		expect(row.expires_at.getTime()).toBe(now.getTime() + SESSION_TTL_MS);
	});

	it('invalidateSession revokes the token', async () => {
		const { token, session } = await createSession(sql, userId);
		await invalidateSession(sql, session.id);
		expect(await validateSessionToken(sql, token)).toBeNull();
	});
});

describe('project settings in postgres', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await migrate(sql);
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	it('recordClaudeState follows the sandbox answer and keeps the first who/when', async () => {
		const s = projectSettings(sql);
		await s.recordClaudeState('web', false, 'alice');
		expect((await s.get('web')).claudeTokenConfigured).toBe(false);

		await s.recordClaudeState('web', true, 'alice');
		const first = await s.get('web');
		expect(first).toMatchObject({ claudeTokenConfigured: true, claudeConfiguredBy: 'alice' });

		await s.recordClaudeState('web', true, 'bob');
		expect(await s.get('web')).toEqual(first);

		await s.recordClaudeState('web', false, 'bob');
		expect(await s.get('web')).toMatchObject({
			claudeTokenConfigured: false,
			claudeConfiguredAt: null,
			claudeConfiguredBy: null
		});
	});
});

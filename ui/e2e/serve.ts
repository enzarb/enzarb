// The e2e web server (Playwright's `webServer.command`, run with node, which strips the types).
//
// Playwright starts the web server before `globalSetup`, so the database cannot be prepared there and
// handed over. Instead this script does it all, in order: start Postgres (a testcontainer, or
// E2E_DATABASE_URL when set), migrate, seed a user and a session, write the browser's storageState,
// then run vite dev with the in-memory project store (vite.e2e.config.ts). Stopping it stops both.
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { migrate } from '../scripts/migrate.ts';
import {
	applyEngineSchema,
	seedBoard,
	seedTask
} from '../src/lib/server/testdata/engine-fixture.ts';
import { SESSION_COOKIE, createSession, upsertUser } from '../src/lib/server/auth.ts';
import { E2E_PORT, E2E_PROJECT, E2E_USER, SERVER_ENV, STORAGE_STATE } from './constants.ts';

const root = path.resolve(import.meta.dirname, '..');

let container: StartedPostgreSqlContainer | undefined;
let databaseUrl = process.env.E2E_DATABASE_URL;
if (!databaseUrl) {
	container = await new PostgreSqlContainer('postgres:18').start();
	databaseUrl = container.getConnectionUri();
}

const sql = postgres(databaseUrl, { max: 1, onnotice: () => {} });
try {
	await migrate(sql);
	// The engine's API with two tasks in the project: one waits on the user (the nav badge and the
	// /tasks queue), one is running.
	await applyEngineSchema(sql);
	await seedBoard(sql, E2E_PROJECT, 'e2e-org/app', {
		columns: [
			{ id: 'doing', title: 'In progress' },
			{ id: 'review', title: 'Review' }
		]
	});
	await seedTask(sql, {
		project: E2E_PROJECT,
		title: 'Add search',
		status: 'waiting',
		column: 'review',
		reason: 'Plan awaiting approval',
		needsYou: true,
		actions: [{ label: 'Approve', command: 'approve' }],
		subjects: [
			{ kind: 'issue', key: 'github:e2e-org/app#1', url: 'https://github.com/e2e-org/app/issues/1' }
		]
	});
	await seedTask(sql, {
		project: E2E_PROJECT,
		title: 'Fix login',
		status: 'running',
		column: 'doing'
	});
	const userId = await upsertUser(sql, {
		id: 4242,
		login: E2E_USER,
		name: 'E2E User',
		avatarUrl: null
	});
	// The user's Claude token (ciphertext is opaque to the UI; only presence is read).
	await sql`INSERT INTO app.claude_credentials (owner, key_id, ciphertext)
		VALUES (${E2E_USER}, 'e2e', '\\x00'::bytea) ON CONFLICT (owner) DO NOTHING`;
	const { token, session } = await createSession(sql, userId);
	const state = {
		cookies: [
			{
				name: SESSION_COOKIE,
				value: token,
				domain: 'localhost',
				path: '/',
				expires: Math.floor(session.expiresAt.getTime() / 1000),
				httpOnly: true,
				secure: false,
				sameSite: 'Lax'
			}
		],
		origins: []
	};
	const file = path.resolve(root, STORAGE_STATE);
	await mkdir(path.dirname(file), { recursive: true });
	await writeFile(file, JSON.stringify(state));
} finally {
	await sql.end();
}

// E2E_PROD=1 serves the production build (`vite build --config vite.e2e.config.ts`) instead of
// vite dev, so client bugs that only show in minified, optimized code are caught too.
const prod = process.env.E2E_PROD === '1';
const vite = spawn(
	process.execPath,
	prod
		? [path.join(root, 'build/index.js')]
		: [
				path.join(root, 'node_modules/vite/bin/vite.js'),
				'dev',
				'--config',
				'vite.e2e.config.ts',
				'--port',
				String(E2E_PORT),
				'--strictPort'
			],
	{
		cwd: root,
		stdio: 'inherit',
		env: {
			...process.env,
			...SERVER_ENV,
			DATABASE_URL: databaseUrl,
			// As in the chart: the origin check reads the scheme from X-Forwarded-Proto, which the
			// browser context sends here (playwright.config.ts) in place of the Gateway.
			...(prod ? { PORT: String(E2E_PORT), PROTOCOL_HEADER: 'x-forwarded-proto' } : {})
		}
	}
);

let stopping = false;
async function stop(code: number) {
	if (stopping) return;
	stopping = true;
	vite.kill('SIGTERM');
	await container?.stop().catch(() => {});
	process.exit(code);
}
vite.on('exit', (code) => void stop(code ?? 1));
process.on('SIGTERM', () => void stop(0));
process.on('SIGINT', () => void stop(0));

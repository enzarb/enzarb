import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { TaskError, chatsService, secretsService, tasksService } from './tasks-service.ts';
import { createItemHandler, loadItemFrom } from './notifier.ts';
import {
	applyEngineSchema,
	seedBoard,
	seedWorkflowView,
	seedSessionEvent,
	seedStepRun,
	seedTask
} from './testdata/engine-fixture.ts';

describe('tasksService against engine_api', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;
	let waiting: number;
	let running: number;
	let theirs: number;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await applyEngineSchema(sql);
		waiting = await seedTask(sql, {
			project: 'web',
			title: 'Add search',
			status: 'waiting',
			column: 'review',
			reason: 'Plan awaiting approval',
			text: 'Approve or comment.',
			needsYou: true,
			actions: [{ label: 'Approve', command: 'approve' }],
			artifacts: [
				{ title: 'Plan', format: 'markdown', content: '## Plan\n- a', node: 'plan/plan' }
			],
			subjects: [
				{ kind: 'issue', key: 'github:acme/web#1', url: 'https://github.com/acme/web/issues/1' }
			]
		});
		running = await seedTask(sql, { project: 'web', title: 'Fix login', column: 'doing' });
		theirs = await seedTask(sql, { project: 'other', title: 'Secret', needsYou: true });
		await seedStepRun(sql, waiting, 'run-1', { cost: 0.25 });
		await seedStepRun(sql, running, 'run-2', {
			ended: false,
			step: 'implement',
			node: 'implement/run'
		});
		await seedSessionEvent(sql, 'run-1', 1, 'prompt', { text: 'Plan the change' });
		await seedSessionEvent(sql, 'run-1', 2, 'assistant', { text: 'Here is the plan' });
		await seedSessionEvent(sql, 'run-1', 3, 'tool_call', { name: 'Read', input: { path: 'x' } });
		await seedBoard(sql, 'web', 'acme/web', {
			columns: [
				{ id: 'doing', title: 'In progress', limit: 3 },
				{ id: 'review', title: 'Review' }
			]
		});
		await seedWorkflowView(sql, 'web', 'acme/web', {
			bindings: [{ id: 'issues', on: 'issue', use: 'std/workflows/issue' }],
			workflows: [],
			lint: [{ level: 'error', message: 'bad' }]
		});
		await sql`INSERT INTO engine.chats (run_id, "user", project, title) VALUES ('chat-1', 'octo', 'web', 'Hi')`;
		await seedSessionEvent(sql, 'chat-1', 1, 'user', { text: 'hello' });
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	const svc = () => tasksService(sql, ['web']);

	it('lists only owned projects', async () => {
		const rows = await svc().list();
		expect(rows.map((r) => r.title).sort()).toEqual(['Add search', 'Fix login']);
		expect(await svc().list('other')).toEqual([]);
		expect(await tasksService(sql, []).list()).toEqual([]);
	});

	it('maps projection rows without artifact content', async () => {
		const row = await svc().get(waiting);
		expect(row).toMatchObject({
			id: waiting,
			status: 'waiting',
			column: 'review',
			needsYou: true,
			reason: 'Plan awaiting approval',
			actions: [{ label: 'Approve', command: 'approve' }],
			artifacts: [{ title: 'Plan', format: 'markdown', node: 'plan/plan' }]
		});
		expect(row?.artifacts[0]).not.toHaveProperty('content');
		expect(await svc().get(theirs)).toBeNull();
	});

	it('lists needs_you rows', async () => {
		expect((await svc().needsYou()).map((r) => r.id)).toEqual([waiting]);
	});

	it('reads the detail with transitions, step runs and artifact content', async () => {
		const d = await svc().detail(waiting);
		expect(d?.artifacts[0].content).toBe('## Plan\n- a');
		expect(d?.stepRuns).toHaveLength(1);
		expect(d?.stepRuns[0]).toMatchObject({ runId: 'run-1', costUsd: 0.25 });
		expect(d?.transitions).toEqual([]);
		expect(await svc().detail(theirs)).toBeNull();
	});

	it('pages a session of an owned run only', async () => {
		const all = await svc().session(waiting, 'run-1');
		expect(all.map((e) => e.kind)).toEqual(['prompt', 'assistant', 'tool_call']);
		expect((await svc().session(waiting, 'run-1', 2)).map((e) => e.seq)).toEqual([3]);
		await expect(svc().session(waiting, 'run-2')).rejects.toBeInstanceOf(TaskError);
		await expect(svc().session(theirs, 'run-1')).rejects.toBeInstanceOf(TaskError);
	});

	it('searches sessions in an owned project', async () => {
		const hits = await svc().search('web', 'plan');
		expect(hits.map((h) => h.seq).sort()).toEqual([1, 2]);
		await expect(svc().search('other', 'x')).rejects.toBeInstanceOf(TaskError);
	});

	it('reads the board and the workflow view', async () => {
		expect((await svc().board('web')).columns.map((c) => c.id)).toEqual(['doing', 'review']);
		expect((await svc().board('web')).columns[0].limit).toBe(3);
		const w = await svc().workflow('web', 'acme/web');
		expect(w?.bindings[0].id).toBe('issues');
		expect(w?.lint).toEqual([{ level: 'error', message: 'bad' }]);
		expect(await svc().workflow('web', 'acme/none')).toBeNull();
	});

	it('aggregates metrics by workflow, phase and step', async () => {
		const rows = await svc().metrics(
			'web',
			new Date(Date.now() - 86_400_000),
			new Date(Date.now() + 60_000)
		);
		expect(rows.map((r) => `${r.phase}/${r.step}`).sort()).toEqual([
			'implement/implement',
			'plan/plan'
		]);
		expect(rows.find((r) => r.step === 'plan')).toMatchObject({
			runs: 1,
			costUsd: 0.25,
			inputTokens: 1000
		});
	});

	it('submits UI events as the actor and refuses foreign tasks', async () => {
		const id = await svc().submit(waiting, { login: 'octo', id: 1 }, 'command', {
			command: 'approve'
		});
		const [e] = await sql`SELECT type, task_id, payload FROM engine.events WHERE id = ${id}`;
		expect(e).toMatchObject({
			type: 'ui.command',
			payload: { source: 'ui', actor: { login: 'octo' }, data: { command: 'approve' } }
		});
		expect(Number(e.task_id)).toBe(waiting);
		await expect(svc().submit(theirs, { login: 'octo', id: 1 }, 'cancel')).rejects.toBeInstanceOf(
			TaskError
		);
	});

	it('stores secrets and lists names only', async () => {
		const s = secretsService(sql);
		await s.set('user', 'octo', 'NPM_TOKEN', 'abc');
		await s.set('user', 'octo', 'NPM_TOKEN', 'def');
		await s.set('project', 'web', 'API_KEY', 'x');
		const names = await s.list('user', 'octo');
		expect(names.map((n) => n.name)).toEqual(['NPM_TOKEN']);
		expect(JSON.stringify(names)).not.toContain('def');
		expect(await s.remove('user', 'octo', 'NPM_TOKEN')).toBe(true);
		expect(await s.list('user', 'octo')).toEqual([]);
		expect((await s.list('project', 'web')).map((n) => n.name)).toEqual(['API_KEY']);
	});

	it('lists the user own chats and reads their sessions', async () => {
		const c = chatsService(sql, 'octo');
		expect((await c.list('web')).map((r) => r.runId)).toEqual(['chat-1']);
		expect((await c.session('chat-1')).map((e) => e.kind)).toEqual(['user']);
		await expect(chatsService(sql, 'mallory').session('chat-1')).rejects.toBeInstanceOf(TaskError);
	});

	it('notifies from projection needs_you', async () => {
		const sent: unknown[] = [];
		const handle = createItemHandler({
			loadItem: loadItemFrom(sql),
			ownerOf: async () => 'octo',
			store: {
				targets: async () => [{ endpoint: 'https://p/1', p256dh: 'k', auth: 'a' }],
				claim: async () => ({ claimed: true, recent: 1 }),
				release: async () => true,
				remove: async () => true,
				touch: async () => {},
				entry: async () => null,
				setEntry: async () => {},
				clearEntry: async () => false
			},
			send: async (_t, p) => {
				sent.push(p);
				return 'ok';
			}
		});
		expect(await handle(waiting)).toBe(true);
		expect(await handle(running)).toBe(false);
		expect(sent).toEqual([expect.objectContaining({ title: 'Plan awaiting approval' })]);
	});
});

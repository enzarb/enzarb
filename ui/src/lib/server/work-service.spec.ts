import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { WorkError, jobOutputMarkdown, workService } from './work-service.ts';
import {
	applyWorkflowsSchema,
	seedItem,
	seedJob,
	seedTransition
} from './testdata/workflows-fixture.ts';

describe('jobOutputMarkdown', () => {
	it('uses the body of the trailing decision block', () => {
		const summary = 'I looked.\n\n```json\n{ "decision": "plan", "body": "## Plan\\n- a" }\n```';
		expect(jobOutputMarkdown({ summary })).toBe('**Decision:** plan\n\n## Plan\n- a');
	});
	it('falls back to the summary text and to errors', () => {
		expect(jobOutputMarkdown({ summary: 'plain' })).toBe('plain');
		expect(jobOutputMarkdown({ summary: '```json\n{oops\n```' })).toContain('oops');
		expect(jobOutputMarkdown({ error: 'boom' })).toBe('**Error:** boom');
		expect(jobOutputMarkdown(null)).toBeNull();
		expect(jobOutputMarkdown({})).toBeNull();
	});
});

describe('workService against workflows_api', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;
	let mine: number;
	let approval: number;
	let failed: number;
	let done: number;
	let theirs: number;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await applyWorkflowsSchema(sql);

		mine = await seedItem(sql, {
			key: 'github:acme/web#1',
			project: 'web',
			state: 'Planning',
			stateData: { title: 'Add search', url: 'https://github.com/acme/web/issues/1' }
		});
		await seedTransition(sql, mine, null, 'Triaging', 'WorkItemOpened');
		await seedTransition(sql, mine, 'Triaging', 'Planning', 'JobSucceeded');
		await seedJob(sql, mine, 'agent_triage', 'succeeded', {
			summary:
				'ok\n```json\n{"decision":"ready","body":"**Enough** info <script>alert(1)</script>"}\n```'
		});
		await seedJob(sql, mine, 'agent_plan', 'running');
		approval = await seedItem(sql, {
			key: 'github:acme/web#2',
			project: 'web',
			state: 'AwaitingApproval'
		});
		failed = await seedItem(sql, { key: 'github:acme/api#3', project: 'api', state: 'Failed' });
		done = await seedItem(sql, { key: 'github:acme/api#4', project: 'api', state: 'Done' });
		theirs = await seedItem(sql, {
			key: 'github:other/x#5',
			project: 'secret',
			state: 'AwaitingApproval'
		});
		await seedItem(sql, { key: 'github:acme/none#6', project: null, state: 'Triaging' });
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	it('lists only owned projects, newest first, optionally filtered', async () => {
		const svc = workService(sql, ['web', 'api']);
		const all = await svc.list();
		expect(all.map((i) => i.key)).not.toContain('github:other/x#5');
		expect(all).toHaveLength(4);
		expect((await svc.list('api')).map((i) => i.project)).toEqual(['api', 'api']);
		expect(await svc.list('secret')).toEqual([]);
		expect(await workService(sql, []).list()).toEqual([]);
	});

	it('derives title and url from state_data, falling back to the key', async () => {
		const items = await workService(sql, ['web']).list('web');
		const one = items.find((i) => i.id === mine)!;
		expect(one.title).toBe('Add search');
		expect(one.url).toBe('https://github.com/acme/web/issues/1');
		const two = items.find((i) => i.id === approval)!;
		expect(two.title).toBe('github:acme/web#2');
		expect(two.url).toBe('https://github.com/acme/web/issues/2');
	});

	it('returns detail with ordered transitions and sanitized job markdown', async () => {
		const d = (await workService(sql, ['web']).get(mine))!;
		expect(d.transitions.map((t) => t.toState)).toEqual(['Triaging', 'Planning']);
		expect(d.transitions[0].fromState).toBeNull();
		expect(d.actions).toEqual(['cancel']);
		expect(d.jobs.map((j) => j.kind)).toEqual(['agent_triage', 'agent_plan']);
		expect(d.jobs[0].outputHtml).toContain('<strong>Enough</strong>');
		expect(d.jobs[0].outputHtml).not.toContain('<script');
		expect(d.jobs[1].outputHtml).toBeNull();
	});

	it('hides other owners’ items', async () => {
		const svc = workService(sql, ['web']);
		expect(await svc.get(theirs)).toBeNull();
		await expect(svc.submitAction(theirs, { login: 'a', id: 1 }, 'cancel')).rejects.toMatchObject({
			kind: 'not-found'
		});
	});

	it('only offers actions valid for the state', async () => {
		const svc = workService(sql, ['web', 'api']);
		expect((await svc.get(approval))!.actions).toEqual(['approve', 'cancel']);
		expect((await svc.get(failed))!.actions).toEqual(['retry', 'cancel']);
		expect((await svc.get(done))!.actions).toEqual([]);
	});

	it('rejects invalid actions for the current state', async () => {
		const svc = workService(sql, ['web', 'api']);
		const actor = { login: 'alice', id: 7 };
		await expect(svc.submitAction(mine, actor, 'approve')).rejects.toBeInstanceOf(WorkError);
		await expect(svc.submitAction(done, actor, 'cancel')).rejects.toMatchObject({
			kind: 'invalid-action'
		});
		await expect(svc.submitAction(approval, actor, 'retry')).rejects.toMatchObject({
			kind: 'invalid-action'
		});
	});

	it('submits through workflows_api.submit_action as a ui event', async () => {
		const svc = workService(sql, ['web']);
		const eventId = await svc.submitAction(approval, { login: 'alice', id: 7 }, 'approve');
		expect(eventId).toBeGreaterThan(0);
		const [e] =
			await sql`SELECT source, type, work_item_key, payload FROM workflows.events WHERE id = ${eventId}`;
		expect(e.source).toBe('ui');
		expect(e.type).toBe('ApprovalGranted');
		expect(e.work_item_key).toBe('github:acme/web#2');
		expect(e.payload).toEqual({
			actor: { login: 'alice', id: 7, is_bot: false },
			action: 'approve',
			data: {}
		});
		await svc.submitAction(approval, { login: 'alice', id: 7 }, 'cancel');
		const types = await sql`SELECT type FROM workflows.events ORDER BY id`;
		expect(types.map((r) => r.type)).toEqual(['ApprovalGranted', 'CancelRequested']);
	});
});

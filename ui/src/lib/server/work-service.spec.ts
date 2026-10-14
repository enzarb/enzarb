import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql';
import postgres from 'postgres';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { classifyNeedsYou } from '#lib/work-items.ts';
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
	let stale: number;
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
		stale = await seedItem(sql, { key: 'github:acme/api#5', project: 'api', state: 'Stale' });
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
		expect(all).toHaveLength(5);
		expect((await svc.list('api')).map((i) => i.project)).toEqual(['api', 'api', 'api']);
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

	it('exposes per-job usage and per-item totals; unknown stays null', async () => {
		const svc = workService(sql, ['web']);
		const before = await svc.get(mine);
		expect(before!.usage).toBeNull();
		expect(before!.jobs.every((j) => j.usage === null)).toBe(true);

		await sql`
			UPDATE workflows.jobs
			SET usage = ${sql.json({ input_tokens: 100, output_tokens: 10, cache_read_input_tokens: 900, models: ['m1'], cost_usd: 0.25 })}
			WHERE work_item_id = ${mine} AND kind = 'agent_triage'`;
		const detail = await svc.get(mine);
		expect(detail!.jobs.find((j) => j.kind === 'agent_triage')!.usage).toEqual({
			inputTokens: 100,
			outputTokens: 10,
			cacheReadTokens: 900,
			cacheWriteTokens: null,
			models: ['m1'],
			costUsd: 0.25
		});
		expect(detail!.jobs.find((j) => j.kind === 'agent_plan')!.usage).toBeNull();
		expect(detail!.usage).toMatchObject({
			inputTokens: 100,
			costUsd: 0.25,
			cacheWriteTokens: null
		});
		const listed = await svc.list('web');
		expect(listed.find((i) => i.id === mine)!.usage?.inputTokens).toBe(100);
		expect(listed.find((i) => i.id === approval)!.usage).toBeNull();
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
		expect((await svc.get(stale))!.actions).toEqual(['retry', 'cancel']);
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

	it('merges only from ReadyToMerge, as the session actor with an empty payload', async () => {
		const id = await seedItem(sql, {
			key: 'github:acme/api#20',
			project: 'api',
			workflow: 'change_request',
			state: 'ReadyToMerge',
			stateData: {
				title: 'feat: x',
				checklist: { ci: { status: 'ok', detail: 'green' } },
				merge_error: ''
			}
		});
		const svc = workService(sql, ['api']);
		const d = (await svc.get(id))!;
		expect(d.actions).toEqual(['merge', 'cancel']);
		expect(d.readiness?.ci).toEqual({ status: 'ok', detail: 'green' });
		expect(d.readiness?.review.status).toBe('unknown');
		expect(d.readiness?.mergeError).toBeNull();
		expect((await svc.get(done))!.readiness).toBeNull();
		await expect(svc.submitAction(done, { login: 'a', id: 1 }, 'merge')).rejects.toMatchObject({
			kind: 'invalid-action'
		});
		await expect(
			workService(sql, ['web']).submitAction(id, { login: 'a', id: 1 }, 'merge')
		).rejects.toMatchObject({ kind: 'not-found' });
		const eventId = await svc.submitAction(id, { login: 'alice', id: 7 }, 'merge');
		const [e] = await sql`SELECT type, payload FROM workflows.events WHERE id = ${eventId}`;
		expect(e.type).toBe('MergeRequested');
		expect(e.payload).toEqual({
			actor: { login: 'alice', id: 7, is_bot: false },
			action: 'merge',
			data: {}
		});
	});

	it('accepts retry from Stale', async () => {
		const id = await seedItem(sql, { key: 'github:acme/api#9', project: 'api', state: 'Stale' });
		const svc = workService(sql, ['api']);
		expect(await svc.submitAction(id, { login: 'alice', id: 7 }, 'retry')).toBeGreaterThan(0);
	});
});

describe('workService needs-you queue', () => {
	let container: StartedPostgreSqlContainer;
	let sql: postgres.Sql;

	beforeAll(async () => {
		container = await new PostgreSqlContainer('postgres:18').start();
		sql = postgres(container.getConnectionUri(), { onnotice: () => {} });
		await applyWorkflowsSchema(sql);
	}, 120_000);

	afterAll(async () => {
		await sql?.end();
		await container?.stop();
	});

	it('lists waiting items of owned projects only, oldest first, beyond 500 newer rows', async () => {
		const oldApproval = await seedItem(sql, {
			key: 'github:acme/q#1',
			project: 'q',
			state: 'AwaitingApproval'
		});
		await sql`UPDATE workflows.work_items SET updated_at = now() - interval '30 days' WHERE id = ${oldApproval}`;
		await sql`
			INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data)
			SELECT 'github:acme/q#' || (100 + n), 'q', 'issue', 'Done', '{}'::jsonb FROM generate_series(1, 520) n`;
		const failed = await seedItem(sql, { key: 'github:acme/q#2', project: 'q', state: 'Failed' });
		const blocked = await seedItem(sql, {
			key: 'github:acme/q#3',
			project: 'q',
			workflow: 'change_request',
			state: 'Reviewing',
			stateData: { checklist: { ci: { status: 'failed', detail: 'tests red' } } }
		});
		await seedItem(sql, { key: 'github:acme/q#4', project: 'q', state: 'Planning' });
		await seedItem(sql, { key: 'github:x/y#1', project: 'other', state: 'AwaitingApproval' });

		const { items: queue, total } = await workService(sql, ['q']).listNeedsYou();
		expect(queue.map((i) => i.id)).toEqual([oldApproval, failed, blocked]);
		expect(total).toBe(3);
		expect(queue.map((i) => i.needs?.category)).toEqual(['approval', 'failed', 'blocked']);
		expect(queue[2].needs?.reason).toBe('CI: tests red');
		expect(await workService(sql, []).listNeedsYou()).toEqual({ items: [], total: 0 });
		expect((await workService(sql, ['other']).listNeedsYou()).items).toHaveLength(1);
	});

	it('caps the queue at 200 in SQL, oldest first, and reports the full total', async () => {
		await sql`
			INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data, updated_at)
			SELECT 'github:acme/cap#' || n, 'cap', 'issue', 'Failed', '{}'::jsonb,
				timestamptz '2026-01-01' + n * interval '1 minute'
			FROM generate_series(1, 205) n`;
		const { items, total } = await workService(sql, ['cap']).listNeedsYou();
		expect(items).toHaveLength(200);
		expect(total).toBe(205);
		expect(items.map((i) => i.key)).toEqual(
			Array.from({ length: 200 }, (_, i) => `github:acme/cap#${i + 1}`)
		);
	});

	it('does not let open change requests without a need crowd out real ones', async () => {
		await sql`
			INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data, updated_at)
			SELECT 'github:acme/crowd#' || n, 'crowd', 'change_request', 'Reviewing',
				'{"checklist":{"ci":{"status":"ok"}}}'::jsonb,
				timestamptz '2026-02-01' + n * interval '1 minute'
			FROM generate_series(1, 250) n`;
		const seed = (n: number, state: string, stateData: unknown, at: string) =>
			seedItem(sql, {
				key: `github:acme/crowd#${1000 + n}`,
				project: 'crowd',
				workflow: 'change_request',
				state,
				stateData
			}).then(async (id) => {
				await sql`UPDATE workflows.work_items SET updated_at = ${at}::timestamptz WHERE id = ${id}`;
				return id;
			});
		const wanted = [
			await seed(1, 'ReadyToMerge', {}, '2026-01-01'),
			await seed(2, 'AwaitingChanges', {}, '2026-01-02'),
			await seed(3, 'Reviewing', { checklist: { ci: { status: 'failed' } } }, '2026-03-01'),
			await seed(4, 'Reviewing', { checklist: { review: { status: 'FAILED' } } }, '2026-03-02')
		];
		const { items, total } = await workService(sql, ['crowd']).listNeedsYou();
		expect(items.map((i) => i.id)).toEqual(wanted);
		expect(total).toBe(4);
	});

	it('selects exactly the rows classifyNeedsYou flags', async () => {
		const cases: [string, string, unknown][] = [
			['issue', 'AwaitingApproval', {}],
			['change_request', 'AwaitingApproval', {}],
			['mention', 'AwaitingApproval', {}],
			['issue', 'ReadyToMerge', {}],
			['change_request', 'ReadyToMerge', {}],
			['change_request', 'AwaitingChanges', {}],
			['issue', 'AwaitingChanges', {}],
			['issue', 'Failed', {}],
			['change_request', 'Stale', {}],
			['mention', 'Failed', {}],
			['issue', 'Planning', {}],
			['change_request', 'Reviewing', {}],
			['change_request', 'Reviewing', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Reviewing', { checklist: { review: { status: 'Failed' } } }],
			['change_request', 'Reviewing', { checklist: { mergeable: { status: 'failed' } } }],
			['change_request', 'Reviewing', { checklist: { ci: { status: 'ok' } } }],
			['issue', 'Reviewing', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Merged', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Closed', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Done', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Cancelled', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Duplicate', { checklist: { ci: { status: 'failed' } } }],
			['change_request', 'Done', {}],
			['issue', 'Failed', []],
			['change_request', 'Reviewing', []],
			['change_request', 'Reviewing', 'failed'],
			['change_request', 'Reviewing', 5],
			['change_request', 'Reviewing', { checklist: [] }],
			['change_request', 'Reviewing', { checklist: 'failed' }],
			['change_request', 'Reviewing', { checklist: { ci: 'failed' } }],
			['change_request', 'Reviewing', { checklist: { ci: ['failed'] } }],
			['change_request', 'Reviewing', { checklist: { ci: { status: 5 } } }],
			['change_request', 'Reviewing', { checklist: { ci: { status: true } } }],
			['change_request', 'Reviewing', { checklist: { ci: { status: null } } }],
			['change_request', 'Reviewing', { checklist: { ci: { status: ['failed'] } } }],
			['change_request', 'Reviewing', { checklist: { ci: { status: { x: 'failed' } } } }],
			// state_data.wait wins over the state-based fallback
			[
				'change_request',
				'AwaitingChanges',
				{ wait: { waiting_on: 'human', reason: 'awaiting_review' } }
			],
			[
				'change_request',
				'AwaitingChanges',
				{ wait: { waiting_on: 'enzarb', reason: 'reviewing' } }
			],
			['change_request', 'AwaitingChanges', { wait: { waiting_on: 'ci', reason: 'ci_pending' } }],
			[
				'change_request',
				'ReadyToMerge',
				{ wait: { waiting_on: 'human', reason: 'ready_to_merge' } }
			],
			['change_request', 'ReadyToMerge', { wait: { waiting_on: 'github', reason: 'merging' } }],
			['change_request', 'Reviewing', { wait: { waiting_on: 'human', reason: 'x' } }],
			[
				'change_request',
				'ConflictParked',
				{ wait: { waiting_on: 'human', reason: 'conflict_parked' } }
			],
			[
				'change_request',
				'Reviewing',
				{ wait: { waiting_on: 'enzarb' }, checklist: { ci: { status: 'failed' } } }
			],
			['change_request', 'AwaitingChanges', { wait: null }],
			['change_request', 'AwaitingChanges', { wait: { waiting_on: 'bogus' } }],
			['change_request', 'AwaitingChanges', { wait: 'human' }],
			['change_request', 'Merged', { wait: { waiting_on: 'human', reason: 'x' } }],
			['issue', 'Reviewing', { wait: { waiting_on: 'human', reason: 'x' } }]
		];
		const ids = new Map<number, number>();
		for (const [i, [workflow, state, stateData]] of cases.entries()) {
			const id = await seedItem(sql, {
				key: `github:acme/parity#${i}`,
				project: 'parity',
				workflow,
				state,
				stateData
			});
			ids.set(id, i);
		}
		const expected = cases
			.map(([workflow, state, stateData], i) => ({
				i,
				needs: classifyNeedsYou({ workflow, state, stateData })
			}))
			.filter((c) => c.needs)
			.map((c) => c.i);
		expect(expected.length).toBeGreaterThan(8);
		const { items, total } = await workService(sql, ['parity']).listNeedsYou();
		expect(items.map((it) => ids.get(it.id)).sort((a, b) => a! - b!)).toEqual(expected);
		expect(total).toBe(expected.length);
	});

	it('returns the latest plan as sanitized html, null when missing or not owned', async () => {
		const id = await seedItem(sql, {
			key: 'github:acme/p#1',
			project: 'p',
			state: 'AwaitingApproval'
		});
		expect(await workService(sql, ['p']).plan(id)).toBeNull();
		await seedJob(sql, id, 'agent_plan', 'succeeded', {
			summary: '```json\n{"decision":"plan","body":"## Plan <script>alert(1)</script>"}\n```'
		});
		const html = await workService(sql, ['p']).plan(id);
		expect(html).toContain('<h2');
		expect(html).not.toContain('<script');
		expect(await workService(sql, ['nope']).plan(id)).toBeNull();
	});
});

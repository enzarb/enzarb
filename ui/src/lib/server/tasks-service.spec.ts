import { describe, expect, it, vi } from 'vitest';
import type { RunDetail, RunSummary } from '#lib/runs.ts';
import { IMPLEMENT_WORKFLOW } from '#lib/tasks.ts';
import { projectTasks, retryRun, startWorkflow } from './tasks-service.ts';
import { TracksError, type TracksClient } from './tracks.ts';

const project = {
	name: 'p',
	repositories: [{ url: 'https://github.com/O/R' }]
} as never;

const run = (over: Partial<RunDetail> = {}): RunDetail => ({
	id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
	workflow: 'fix',
	project: 'p',
	repo: 'o/r',
	subject: { kind: 'issue', number: 3 },
	eventId: 1,
	sha: 'abc',
	status: 'done',
	test: false,
	currentNode: null,
	reason: null,
	stopRequested: false,
	createdAt: '2026-10-30T12:00:00.000Z',
	updatedAt: '2026-10-30T12:00:00.000Z',
	finishedAt: null,
	nodeRuns: [],
	...over
});

function fakeTracks(runs: RunDetail[]) {
	return {
		runs: vi.fn(async () => runs as RunSummary[]),
		run: vi.fn(async (id: string) => runs.find((r) => r.id === id) ?? null),
		startRun: vi.fn(async () => ({ id: 'new', alreadyRunning: false }))
	} as unknown as TracksClient & { startRun: ReturnType<typeof vi.fn> };
}

const github = (
	pulls: unknown[] = [],
	issues: unknown[] = [],
	note?: string,
	open: number[] | null = null
) => ({
	openSubjects: vi.fn(async () => ({ open: open ? new Set(open) : null })),
	pulls: vi.fn(async () => ({ items: pulls as never[], note })),
	issues: vi.fn(async () => ({ items: issues as never[] }))
});

describe('projectTasks', () => {
	it('lists parked, failed, review and triaged tasks', async () => {
		const tracks = fakeTracks([
			run({
				id: '11111111-1111-4111-8111-111111111111',
				status: 'waiting',
				workflow: 'ask',
				currentNode: 'w',
				waitUntil: '2026-11-01T00:00:00.000Z',
				nodeRuns: [
					{
						node: 'w',
						visit: 1,
						status: 'waiting',
						rendered: { type: 'wait', on: ['issue_comment.created'] },
						wait: {
							on: ['issue_comment.created'],
							if: "event.x == 'y'",
							timeout_secs: 1,
							until: null
						}
					} as never
				]
			}),
			run({ id: '22222222-2222-4222-8222-222222222222', status: 'failed', reason: 'timeout' })
		]);
		const gh = github(
			[
				{
					number: 8,
					title: 'PR',
					url: 'https://github.com/o/r/pull/8',
					branch: 'ez/fix/22222222-2222-4222-8222-222222222222',
					draft: false,
					updatedAt: '2026-10-30T13:00:00.000Z'
				}
			],
			[{ number: 5, title: 'Bug', body: '', url: 'u', updatedAt: '2026-10-30T14:00:00.000Z' }]
		);
		const { tasks, notes } = await projectTasks({ tracks, github: gh }, project);
		expect(notes).toEqual([]);
		const byKind = Object.fromEntries(tasks.map((t) => [t.kind, t]));
		expect(byKind.wait).toMatchObject({
			waitingFor: "issue_comment.created if event.x == 'y'",
			until: '2026-11-01T00:00:00.000Z',
			runUrl: '/projects/p/runs/11111111-1111-4111-8111-111111111111'
		});
		expect(byKind.failed).toMatchObject({
			reason: 'timeout',
			githubUrl: 'https://github.com/o/r/issues/3'
		});
		expect(byKind.review).toMatchObject({
			number: 8,
			runUrl: '/projects/p/runs/22222222-2222-4222-8222-222222222222'
		});
		expect(byKind.triaged).toMatchObject({ number: 5, repo: 'o/r' });
		expect(gh.issues).toHaveBeenCalledWith('o/r', 'triage');
	});

	it('leaves out triaged issues that have an implement run, and failures that were retried', async () => {
		const tracks = fakeTracks([
			run({
				id: '33333333-3333-4333-8333-333333333333',
				workflow: IMPLEMENT_WORKFLOW,
				status: 'running',
				subject: { kind: 'issue', number: 5 }
			}),
			// Newest first: the retry is running, the older failure no longer needs you.
			run({ id: '44444444-4444-4444-8444-444444444444', workflow: 'fix', status: 'running' }),
			run({ id: '55555555-5555-4555-8555-555555555555', workflow: 'fix', status: 'failed' })
		]);
		const gh = github(
			[],
			[{ number: 5, title: 'Bug', body: '', url: 'u', updatedAt: '2026-10-30T14:00:00.000Z' }]
		);
		const { tasks } = await projectTasks({ tracks, github: gh }, project);
		expect(tasks).toEqual([]);
	});

	it('hides failed and waiting runs whose subject is closed, keeps open and unknown ones', async () => {
		const failed = (id: string, n: number) =>
			run({
				id,
				workflow: `w${n}`,
				status: 'failed',
				subject: { kind: 'pull_request', number: n }
			});
		const mk = () =>
			fakeTracks([
				failed('aaaaaaaa-0000-4000-8000-000000000001', 1),
				failed('aaaaaaaa-0000-4000-8000-000000000002', 2),
				run({
					id: 'aaaaaaaa-0000-4000-8000-000000000003',
					status: 'waiting',
					workflow: 'w3',
					subject: { kind: 'issue', number: 3 }
				})
			]);
		const shown = async (open: number[] | null) =>
			(await projectTasks({ tracks: mk(), github: github([], [], undefined, open) }, project)).tasks
				.map((t) => t.number)
				.sort();
		expect(await shown([2])).toEqual([2]);
		expect(await shown([])).toEqual([]);
		expect(await shown(null)).toEqual([1, 2, 3]);
	});

	it('takes the first (newest) run per subject and workflow: an older failure is not shown', async () => {
		// tracks orders runs by created_at DESC (tracks/src/runs_api.rs).
		const tracks = fakeTracks([
			run({ id: '66666666-6666-4666-8666-666666666666', workflow: 'fix', status: 'done' }),
			run({ id: '77777777-7777-4777-8777-777777777777', workflow: 'fix', status: 'failed' })
		]);
		const { tasks } = await projectTasks({ tracks, github: github() }, project);
		expect(tasks.filter((t) => t.kind === 'failed')).toEqual([]);
	});

	it('picks the newest run by createdAt even when the input is oldest first', async () => {
		const tracks = fakeTracks([
			run({
				id: '88888888-8888-4888-8888-888888888888',
				workflow: 'fix',
				status: 'failed',
				createdAt: '2026-10-30T10:00:00.000Z'
			}),
			run({
				id: '99999999-9999-4999-8999-999999999999',
				workflow: 'fix',
				status: 'done',
				createdAt: '2026-10-30T11:00:00.000Z'
			})
		]);
		const { tasks } = await projectTasks({ tracks, github: github() }, project);
		expect(tasks.filter((t) => t.kind === 'failed')).toEqual([]);
	});

	it('carries notes for GitHub limits and a tracks outage', async () => {
		const tracks = fakeTracks([]);
		tracks.runs = vi.fn(async () => {
			throw new TracksError(503, 'unavailable', 'down');
		});
		const { tasks, notes } = await projectTasks(
			{ tracks, github: github([], [], 'GitHub rate limit: o/r is shown from memory') },
			project
		);
		expect(tasks).toEqual([]);
		expect(notes).toHaveLength(2);
	});
});

describe('actions', () => {
	const allow = vi.fn(async () => ({}) as never);
	const deps = (tracks: TracksClient) => ({ tracks, checkAccess: allow });

	it('Implement starts the workflow after a live write check', async () => {
		const tracks = fakeTracks([]);
		const r = await startWorkflow(deps(tracks), 'bob', project, {
			repo: 'o/r',
			number: 5,
			workflow: IMPLEMENT_WORKFLOW
		});
		expect(r).toMatchObject({ ok: true, runId: 'new' });
		expect(allow).toHaveBeenCalledWith('bob', 'github:o/r');
		expect(tracks.startRun).toHaveBeenCalledWith({
			repo: 'o/r',
			number: 5,
			workflow: IMPLEMENT_WORKFLOW,
			actor: 'bob'
		});
	});

	it('refuses without write access or for a foreign repository', async () => {
		const tracks = fakeTracks([]);
		const input = { repo: 'o/r', number: 5, workflow: 'w' };
		const none = { tracks, checkAccess: vi.fn(async () => null) };
		expect(await startWorkflow(none, 'bob', project, input)).toMatchObject({ ok: false });
		expect(
			await startWorkflow(deps(tracks), 'bob', project, { ...input, repo: 'x/y' })
		).toMatchObject({ ok: false });
		expect(tracks.startRun).not.toHaveBeenCalled();
	});

	it('reports an already running subject and tracks errors', async () => {
		const tracks = fakeTracks([]);
		tracks.startRun.mockResolvedValueOnce({ id: null, alreadyRunning: true });
		const input = { repo: 'o/r', number: 5, workflow: 'w' };
		expect(await startWorkflow(deps(tracks), 'bob', project, input)).toMatchObject({
			ok: true,
			runId: null
		});
		tracks.startRun.mockRejectedValueOnce(
			new TracksError(422, 'not_startable', '`w` cannot be started by hand')
		);
		expect(await startWorkflow(deps(tracks), 'bob', project, input)).toEqual({
			ok: false,
			message: '`w` cannot be started by hand'
		});
	});

	it('Retry starts the same workflow on the same subject, only for failed runs', async () => {
		const failed = run({
			id: '66666666-6666-4666-8666-666666666666',
			status: 'failed',
			workflow: 'wf'
		});
		const done = run({ id: '77777777-7777-4777-8777-777777777777', status: 'done' });
		const tracks = fakeTracks([failed, done]);
		expect(await retryRun(deps(tracks), 'bob', project, failed.id)).toMatchObject({ ok: true });
		expect(tracks.startRun).toHaveBeenCalledWith({
			repo: 'o/r',
			number: 3,
			workflow: 'wf',
			actor: 'bob'
		});
		expect(await retryRun(deps(tracks), 'bob', project, done.id)).toMatchObject({ ok: false });
		expect(await retryRun(deps(tracks), 'bob', project, 'nope')).toMatchObject({ ok: false });
	});
});

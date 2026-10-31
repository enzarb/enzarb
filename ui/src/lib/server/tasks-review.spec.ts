import { describe, expect, it, vi } from 'vitest';
import type { PullState } from '#lib/pull-review.ts';
import type { RunDetail } from '#lib/runs.ts';
import {
	answerPlan,
	approveAndMerge,
	planOfRun,
	pullReview,
	reviewNotes,
	type ReviewDeps
} from './tasks-review.ts';
import { TracksError } from './tracks.ts';

const project = { name: 'p', repositories: [{ url: 'https://github.com/O/R' }] } as never;
const ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';

const run = (over: Partial<RunDetail> = {}): RunDetail =>
	({
		id: ID,
		workflow: 'implement',
		project: 'p',
		repo: 'o/r',
		subject: { kind: 'issue', number: 3 },
		status: 'waiting',
		test: false,
		createdAt: '2026-10-30T12:00:00.000Z',
		nodeRuns: [
			{ node: 'plan', visit: 1, status: 'done', outputs: { result: { plan: '## Plan' } } },
			{ node: 'gate', visit: 1, status: 'waiting', outputs: null }
		],
		...over
	}) as unknown as RunDetail;

const pull: PullState = {
	number: 7,
	title: 'Fix crash',
	url: '',
	draft: false,
	headSha: 'sha1',
	mergeable: true,
	mergeableState: 'clean'
};

function deps(
	over: Partial<{
		run: RunDetail | null;
		resume: unknown;
		allow: boolean;
		pull: PullState | null;
		checks: unknown[];
		approve: unknown;
	}> = {}
) {
	const resumeRun = vi.fn(async () => over.resume ?? { result: 'ok' });
	const checkAccess = vi.fn(async () => (over.allow === false ? null : ({} as never)));
	const writes = {
		approve: vi.fn(async () => (over.approve as never) ?? { ok: true }),
		comment: vi.fn(async () => ({ ok: true }) as const),
		squashMerge: vi.fn(async () => ({ ok: true }) as const)
	};
	const d = {
		tracks: {
			run: vi.fn(async () => (over.run === undefined ? run() : over.run)),
			runs: vi.fn(async () => []),
			resumeRun
		},
		checkAccess,
		writes,
		github: {
			pull: vi.fn(async () => ({ pull: over.pull === undefined ? pull : over.pull })),
			checkRuns: vi.fn(async () => ({
				items: (over.checks ?? [
					{ name: 'build', status: 'completed', conclusion: 'success', url: '' }
				]) as never[]
			})),
			pullFiles: vi.fn(async () => ({ items: [] })),
			markedComments: vi.fn(async () => ({ items: [] }))
		}
	};
	return d as typeof d & ReviewDeps;
}

describe('planOfRun', () => {
	it('renders the plan as sanitised markdown', async () => {
		const d = deps({
			run: run({
				nodeRuns: [
					{
						node: 'plan',
						status: 'done',
						outputs: { result: { plan: '**x**<script>1</script>' } }
					},
					{ node: 'gate', status: 'waiting' }
				] as never
			})
		});
		const p = await planOfRun(d, 'p', ID);
		expect(p?.html).toContain('<strong>x</strong>');
		expect(p?.html).not.toContain('<script');
	});
	it('is null for another Project', async () => {
		expect(await planOfRun(deps(), 'other', ID)).toBeNull();
	});
});

describe('answerPlan', () => {
	it('checks write access live, then resumes as the user', async () => {
		const d = deps();
		const r = await answerPlan(d, 'octo', project, { id: ID, action: 'approve' });
		expect(d.checkAccess).toHaveBeenCalledWith('octo', 'github:o/r');
		expect(d.tracks.resumeRun).toHaveBeenCalledWith({ id: ID, actor: 'octo', action: 'approve' });
		expect(r).toEqual({ ok: true, message: 'Plan approved: implementation started.', runId: ID });
	});
	it('sends the revision text', async () => {
		const d = deps();
		const r = await answerPlan(d, 'octo', project, {
			id: ID,
			action: 'revise',
			text: ' more tests '
		});
		expect(d.tracks.resumeRun).toHaveBeenCalledWith({
			id: ID,
			actor: 'octo',
			action: 'revise',
			text: 'more tests'
		});
		expect(r.message).toMatch(/Revision sent/);
	});
	it('does not resume without write access, and asks for revision text', async () => {
		const d = deps({ allow: false });
		const r = await answerPlan(d, 'octo', project, { id: ID, action: 'approve' });
		expect(r.ok).toBe(false);
		expect(d.tracks.resumeRun).not.toHaveBeenCalled();
		const e = await answerPlan(deps(), 'octo', project, { id: ID, action: 'revise', text: ' ' });
		expect(e.message).toMatch(/Say what to change/);
	});
	it('refuses a run that is not at a plan approval or belongs elsewhere', async () => {
		const d = deps({ run: run({ status: 'running' }) });
		expect((await answerPlan(d, 'octo', project, { id: ID, action: 'approve' })).message).toMatch(
			/not waiting at a plan/
		);
		const o = deps({ run: run({ project: 'other' }) });
		expect((await answerPlan(o, 'octo', project, { id: ID, action: 'approve' })).message).toBe(
			'Run not found.'
		);
		expect(o.checkAccess).not.toHaveBeenCalled();
	});
	it('maps not_waiting and not_accepted to clear messages', async () => {
		const a = await answerPlan(deps({ resume: { result: 'not_waiting' } }), 'o', project, {
			id: ID,
			action: 'approve'
		});
		expect(a).toEqual({
			ok: false,
			message: 'The run is no longer waiting, so nothing was sent. Reload to see where it is.'
		});
		const b = await answerPlan(
			deps({ resume: { result: 'not_accepted', message: 'if refused' } }),
			'o',
			project,
			{ id: ID, action: 'revise', text: 'x' }
		);
		expect(b.message).toBe('The run does not accept a revision right now: if refused');
	});
	it('reports an unreachable tracks', async () => {
		const d = deps();
		d.tracks.resumeRun.mockRejectedValueOnce(new TracksError(503, 'unavailable', 'down'));
		expect((await answerPlan(d, 'o', project, { id: ID, action: 'approve' })).message).toBe(
			'enzarb-tracks could not be reached.'
		);
	});
});

describe('reviewNotes', () => {
	const review = (id: string, at: string, summary: string | null) =>
		run({
			id,
			workflow: 'review-pull-requests',
			subject: { kind: 'pull_request', number: 7 },
			status: 'done',
			createdAt: at,
			nodeRuns: [
				{ node: 'review', status: 'done', outputs: summary ? { result: { summary } } : null }
			] as never
		});

	it('uses the newest review run summary', async () => {
		const runs = [
			review('old', '2026-10-30T10:00:00Z', 'old notes'),
			review('new', '2026-10-30T11:00:00Z', 'new notes'),
			run({ id: 'other', workflow: 'implement' })
		];
		const d = deps();
		d.tracks.runs.mockResolvedValue(runs as never);
		d.tracks.run.mockImplementation(
			(async (id: string) => runs.find((r) => r.id === id) ?? null) as never
		);
		const n = await reviewNotes(d, 'p', 'o/r', 7);
		expect(n).toMatchObject({
			source: 'run',
			run: { id: 'new', workflow: 'review-pull-requests' }
		});
		expect(n?.html).toContain('new notes');
	});
	it('falls back to the newest ez-tracks marked comment, without the marker', async () => {
		const d = deps();
		d.github.markedComments.mockResolvedValue({
			items: ['first\n<!-- ez-tracks:r:n:1 -->', 'second **bold**\n<!-- ez-tracks:r:n:2 -->']
		} as never);
		const n = await reviewNotes(d, 'p', 'o/r', 7);
		expect(n?.source).toBe('comment');
		expect(n?.html).toContain('<strong>bold</strong>');
		expect(n?.html).not.toContain('ez-tracks');
		expect(n?.html).not.toContain('first');
	});
	it('is null with neither', async () => {
		expect(await reviewNotes(deps(), 'p', 'o/r', 7)).toBeNull();
	});
});

describe('pullReview', () => {
	it('refuses repositories outside the Project', async () => {
		const r = await pullReview(deps(), project, 'x/y', 1);
		expect(r.problems[0]).toMatch(/not in this Project/);
	});
});

describe('approveAndMerge', () => {
	const input = { repo: 'o/r', number: 7 };

	it('checks access, approves naming the user, checks status, squash-merges with the title', async () => {
		const d = deps();
		const r = await approveAndMerge(d, 'octo', project, input);
		expect(r.ok).toBe(true);
		expect(r.steps.map((s) => s.result)).toEqual(['done', 'done', 'done', 'done']);
		expect(d.writes.approve).toHaveBeenCalledWith('o/r', 7, 'Approved by @octo in enzarb', 'sha1');
		expect(d.writes.squashMerge).toHaveBeenCalledWith('o/r', 7, {
			title: 'Fix crash (#7)',
			sha: 'sha1'
		});
		const order = [d.checkAccess, d.writes.approve, d.github.checkRuns, d.writes.squashMerge].map(
			(f) => f.mock.invocationCallOrder[0]!
		);
		expect(order).toEqual([...order].sort((a, b) => a - b));
	});

	it('writes nothing without write access', async () => {
		const d = deps({ allow: false });
		const r = await approveAndMerge(d, 'octo', project, input);
		expect(r.ok).toBe(false);
		expect(r.steps[0]!.result).toBe('failed');
		expect(d.writes.approve).not.toHaveBeenCalled();
		expect(d.writes.squashMerge).not.toHaveBeenCalled();
	});

	it.each([
		[
			'checks are red',
			{ checks: [{ name: 'lint', status: 'completed', conclusion: 'failure' }] },
			/failing: lint/
		],
		[
			'checks are pending',
			{ checks: [{ name: 'e2e', status: 'queued', conclusion: null }] },
			/still running: e2e/
		],
		[
			'there are conflicts',
			{ pull: { ...pull, mergeable: false, mergeableState: 'dirty' } },
			/conflicts/
		]
	])('refuses to merge when %s, after approving', async (_n, over, reason) => {
		const d = deps(over as never);
		const r = await approveAndMerge(d, 'octo', project, input);
		expect(r.ok).toBe(false);
		expect(r.message).toMatch(reason);
		expect(r.steps.map((s) => s.result)).toEqual(['done', 'done', 'failed', 'skipped']);
		expect(d.writes.squashMerge).not.toHaveBeenCalled();
	});

	it('records the approval as a comment when the App may not approve its own pull request', async () => {
		const d = deps({
			approve: { ok: false, status: 422, message: 'Can not approve your own pull request' }
		});
		const r = await approveAndMerge(d, 'octo', project, input);
		expect(r.ok).toBe(true);
		expect(d.writes.comment).toHaveBeenCalledWith('o/r', 7, 'Approved by @octo in enzarb');
		expect(r.steps[1]!.message).toMatch(/comment/);
	});

	it('stops when GitHub refuses the approval or the merge', async () => {
		const a = deps({ approve: { ok: false, status: 403, message: 'nope' } });
		const ra = await approveAndMerge(a, 'octo', project, input);
		expect(ra.steps[1]!.result).toBe('failed');
		expect(a.writes.squashMerge).not.toHaveBeenCalled();
		const m = deps();
		m.writes.squashMerge.mockResolvedValueOnce({
			ok: false,
			status: 405,
			message: 'Required reviews'
		} as never);
		const rm = await approveAndMerge(m, 'octo', project, input);
		expect(rm.message).toMatch(/Required reviews/);
		expect(rm.steps[3]!.result).toBe('failed');
	});

	it('refuses a repository outside the Project before any write', async () => {
		const d = deps();
		const r = await approveAndMerge(d, 'octo', project, { repo: 'x/y', number: 1 });
		expect(r.steps[0]!.result).toBe('failed');
		expect(d.writes.approve).not.toHaveBeenCalled();
	});
});

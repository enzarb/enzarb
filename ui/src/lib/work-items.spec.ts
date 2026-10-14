import { describe, expect, it } from 'vitest';
import {
	applyItemChange,
	applyNeedsYouChange,
	availableActions,
	blockedReason,
	classifyNeedsYou,
	countPossiblyRunning,
	groupWorkItems,
	isTerminal,
	itemUrl,
	keyToUrl,
	parseReadiness,
	parseWorkId,
	primaryAction,
	shouldRefresh,
	stateTone,
	waitingFor,
	waitingLabel,
	type NeedsYou,
	type WorkItemSummary
} from './work-items.ts';

describe('needs-you classification', () => {
	const cr = (state: string, stateData: unknown = {}) => ({
		workflow: 'change_request',
		state,
		stateData
	});
	it('classifies each category', () => {
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'AwaitingApproval', stateData: {} })
		).toEqual({ category: 'approval', reason: null });
		expect(classifyNeedsYou(cr('ReadyToMerge'))?.category).toBe('merge');
		expect(classifyNeedsYou(cr('AwaitingChanges'))).toEqual({
			category: 'blocked',
			reason: 'Changes requested'
		});
		expect(
			classifyNeedsYou(cr('Reviewing', { checklist: { ci: { status: 'failed', detail: 'red' } } }))
		).toEqual({ category: 'blocked', reason: 'CI: red' });
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'Failed', stateData: { error: ' boom ' } })
		).toEqual({ category: 'failed', reason: 'boom' });
		expect(classifyNeedsYou({ workflow: 'issue', state: 'Stale', stateData: null })?.category).toBe(
			'failed'
		);
	});
	it('labels why a change request is waiting and keeps round_limit in the queue', () => {
		expect(waitingLabel({ waiting_reason: 'human_review' })).toBe('Waiting for a human review');
		expect(waitingLabel({ waiting_reason: 'enzarb_fixing' })).toBe(
			'enzarb is fixing review findings'
		);
		expect(waitingLabel({ waiting_reason: 'round_limit' })).toMatch(/^Round limit reached/);
		expect(waitingLabel({ waiting_reason: '' })).toBeNull();
		expect(waitingLabel({ waiting_reason: 'nope' })).toBeNull();
		expect(waitingLabel(null)).toBeNull();
		expect(classifyNeedsYou(cr('AwaitingChanges', { waiting_reason: 'round_limit' }))).toEqual({
			category: 'blocked',
			reason: waitingLabel({ waiting_reason: 'round_limit' })
		});
		expect(classifyNeedsYou(cr('AwaitingChanges', { waiting_reason: 'human_review' }))).toEqual({
			category: 'blocked',
			reason: 'Waiting for a human review'
		});
		// enzarb is working on it: nobody needs to act.
		expect(
			classifyNeedsYou(cr('AddressingComments', { waiting_reason: 'enzarb_fixing' }))
		).toBeNull();
	});
	it('ignores terminal, unknown and unrelated items and malformed data', () => {
		const failedCi = { checklist: { ci: { status: 'failed' } } };
		expect(classifyNeedsYou(cr('Merged', failedCi))).toBeNull();
		expect(classifyNeedsYou(cr('Mystery'))).toBeNull();
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'Planning', stateData: failedCi })
		).toBeNull();
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'ReadyToMerge', stateData: {} })
		).toBeNull();
		expect(classifyNeedsYou(cr('Reviewing', 'garbage'))).toBeNull();
	});
	it('blockedReason prefers failed checks, then the merge error', () => {
		expect(blockedReason({ checklist: { mergeable: { status: 'failed' } } })).toBe('Merge failed');
		expect(blockedReason({ merge_error: ' conflict ' })).toBe('conflict');
		expect(blockedReason(null)).toBeNull();
	});
	it('primaryAction maps categories', () => {
		expect(primaryAction('approval')).toBe('approve');
		expect(primaryAction('merge')).toBe('merge');
		expect(primaryAction('failed')).toBe('retry');
		expect(primaryAction('blocked')).toBeNull();
	});
	it('applyNeedsYouChange inserts, replaces, orders oldest first and removes', () => {
		const mk = (id: number, updatedAt: string, needs: NeedsYou | null): WorkItemSummary => ({
			id,
			key: `k${id}`,
			project: 'p',
			workflow: 'issue',
			state: 's',
			title: 't',
			url: null,
			updatedAt,
			needs
		});
		const n: NeedsYou = { category: 'approval', reason: null };
		let list = applyNeedsYouChange([], mk(2, '2026-01-02T00:00:00Z', n));
		list = applyNeedsYouChange(list, mk(1, '2026-01-01T00:00:00Z', n));
		expect(list.map((i) => i.id)).toEqual([1, 2]);
		list = applyNeedsYouChange(list, mk(1, '2026-01-03T00:00:00Z', n));
		expect(list.map((i) => i.id)).toEqual([2, 1]);
		list = applyNeedsYouChange(list, mk(2, '2026-01-04T00:00:00Z', null));
		expect(list.map((i) => i.id)).toEqual([1]);
	});
	it('waitingFor renders a short age', () => {
		const now = Date.parse('2026-01-10T00:00:00Z');
		expect(waitingFor('2026-01-09T23:59:50Z', now)).toBe('<1m');
		expect(waitingFor('2026-01-09T23:15:00Z', now)).toBe('45m');
		expect(waitingFor('2026-01-09T21:00:00Z', now)).toBe('3h');
		expect(waitingFor('2026-01-05T00:00:00Z', now)).toBe('5d');
		expect(waitingFor('nonsense', now)).toBe('<1m');
	});
});

describe('shouldRefresh', () => {
	it('is true for a full refresh and the matching id only', () => {
		expect(shouldRefresh(null, 3)).toBe(true);
		expect(shouldRefresh(3, 3)).toBe(true);
		expect(shouldRefresh(4, 3)).toBe(false);
	});
});

describe('applyItemChange', () => {
	const mk = (id: number, updatedAt: string, project = 'p'): WorkItemSummary => ({
		id,
		key: `github:o/r#${id}`,
		project,
		workflow: 'issue',
		state: 'Working',
		title: `t${id}`,
		url: null,
		updatedAt
	});
	const a = mk(1, '2026-10-11T01:00:00Z');
	const b = mk(2, '2026-10-11T02:00:00Z');

	it('updates a row in place and moves it to its sorted position', () => {
		const changed = { ...a, state: 'Done', updatedAt: '2026-10-11T03:00:00Z' };
		expect(applyItemChange([b, a], changed)).toEqual([changed, b]);
	});

	it('inserts a new item in order', () => {
		const c = mk(3, '2026-10-11T01:30:00Z');
		expect(applyItemChange([b, a], c).map((w) => w.id)).toEqual([2, 3, 1]);
	});

	it('breaks ties by id descending', () => {
		const c = mk(3, a.updatedAt);
		expect(applyItemChange([a], c).map((w) => w.id)).toEqual([3, 1]);
	});

	it('ignores items outside the project filter and drops stale rows', () => {
		const other = mk(5, '2026-10-11T04:00:00Z', 'q');
		expect(applyItemChange([b, a], other, 'p')).toEqual([b, a]);
		expect(applyItemChange([b, a], { ...a, project: 'q' }, 'p')).toEqual([b]);
	});
});

describe('merge', () => {
	it('is offered only in ReadyToMerge, next to cancel', () => {
		expect(availableActions('ReadyToMerge')).toEqual(['merge', 'cancel']);
		expect(availableActions('Merging')).toEqual(['cancel']);
		for (const s of ['Reviewing', 'AwaitingChanges', 'Failed', 'Merged', 'SomethingNew'])
			expect(availableActions(s)).not.toContain('merge');
	});
	it('has a ready tone; unknown states stay active', () => {
		expect(stateTone('ReadyToMerge')).toBe('ready');
		expect(stateTone('FutureState')).toBe('active');
	});
	it('groups ReadyToMerge first, preserving order', () => {
		const items = [
			{ id: 1, state: 'Planning' },
			{ id: 2, state: 'ReadyToMerge' },
			{ id: 3, state: 'Done' },
			{ id: 4, state: 'ReadyToMerge' }
		];
		const g = groupWorkItems(items);
		expect(g.ready.map((i) => i.id)).toEqual([2, 4]);
		expect(g.rest.map((i) => i.id)).toEqual([1, 3]);
	});
});

describe('parseReadiness', () => {
	const unknown = { status: 'unknown', detail: null };
	it('reads checklist statuses and details', () => {
		const r = parseReadiness({
			checklist: {
				ci: { status: 'ok', detail: 'all green' },
				review: { status: 'pending', detail: 'waiting' },
				mergeable: { status: 'failed', detail: 'conflicts' }
			}
		});
		expect(r.ci).toEqual({ status: 'ok', detail: 'all green' });
		expect(r.review).toEqual({ status: 'pending', detail: 'waiting' });
		expect(r.mergeable).toEqual({ status: 'failed', detail: 'conflicts' });
		expect(r.mergeError).toBeNull();
	});
	it('accepts mixed case and a missing detail', () => {
		const r = parseReadiness({
			checklist: { ci: { status: 'OK' }, review: { status: 'Unknown' } }
		});
		expect(r.ci).toEqual({ status: 'ok', detail: null });
		expect(r.review).toEqual(unknown);
		expect(r.mergeable).toEqual(unknown);
	});
	it('reads anything else as unknown', () => {
		for (const bad of [null, 'x', 3, [], {}, { readiness: { ci: { status: 'ok' } } }]) {
			const r = parseReadiness(bad);
			expect([r.ci, r.review, r.mergeable]).toEqual([unknown, unknown, unknown]);
		}
		expect(parseReadiness({ checklist: { ci: { status: 'weird' } } }).ci).toEqual(unknown);
	});
	it('exposes a non-empty merge_error only', () => {
		expect(parseReadiness({ merge_error: ' head moved ' }).mergeError).toBe('head moved');
		for (const v of ['', '  ', null, 4, undefined])
			expect(parseReadiness({ merge_error: v }).mergeError).toBeNull();
	});
});

describe('availableActions', () => {
	it('approve only in AwaitingApproval', () => {
		expect(availableActions('AwaitingApproval')).toEqual(['approve', 'cancel']);
		for (const s of ['Triaging', 'Planning', 'Implementing', 'Failed', 'Done'])
			expect(availableActions(s)).not.toContain('approve');
	});
	it('retry only in Failed and Stale', () => {
		expect(availableActions('Failed')).toEqual(['retry', 'cancel']);
		expect(availableActions('Stale')).toEqual(['retry', 'cancel']);
		for (const s of ['Triaging', 'AwaitingApproval', 'Cancelled'])
			expect(availableActions(s)).not.toContain('retry');
	});
	it('cancel for every non-terminal state, none for terminal ones', () => {
		for (const s of [
			'Triaging',
			'AwaitingAnswers',
			'Stale',
			'Reviewing',
			'AddressingComments',
			'Running'
		])
			expect(availableActions(s)).toContain('cancel');
		for (const s of ['Done', 'Duplicate', 'Cancelled', 'Merged', 'Closed']) {
			expect(isTerminal(s)).toBe(true);
			expect(availableActions(s)).toEqual([]);
		}
	});
});

describe('helpers', () => {
	it('maps states to tones', () => {
		expect(stateTone('Failed')).toBe('failed');
		expect(stateTone('Done')).toBe('done');
		expect(stateTone('AwaitingApproval')).toBe('waiting');
		expect(stateTone('Planning')).toBe('active');
	});
	it('derives GitHub URLs', () => {
		expect(keyToUrl('github:a/b#3')).toBe('https://github.com/a/b/issues/3');
		expect(keyToUrl('gitlab:a/b!3')).toBeNull();
		expect(itemUrl('github:a/b#3', { url: 'https://github.com/a/b/pull/3' })).toBe(
			'https://github.com/a/b/pull/3'
		);
		expect(itemUrl('github:a/b#3', { url: 'javascript:alert(1)' })).toBe(
			'https://github.com/a/b/issues/3'
		);
	});
});

describe('countPossiblyRunning', () => {
	it('counts active states and skips waiting, ready and terminal ones', () => {
		const states = [
			'Planning',
			'Implementing',
			'AwaitingApproval',
			'Stale',
			'ReadyToMerge',
			'Done'
		];
		expect(countPossiblyRunning(states.map((state) => ({ state })))).toBe(2);
		expect(countPossiblyRunning([])).toBe(0);
	});
});

describe('parseWorkId', () => {
	it('accepts canonical positive integers', () => {
		expect(parseWorkId('1')).toBe(1);
		expect(parseWorkId('42')).toBe(42);
	});

	it.each([
		undefined,
		'',
		'abc',
		'0',
		'-3',
		'1.5',
		'1e3',
		' 7',
		'007',
		`${Number.MAX_SAFE_INTEGER}0`
	])('rejects %j', (param) => {
		expect(parseWorkId(param)).toBeNull();
	});
});

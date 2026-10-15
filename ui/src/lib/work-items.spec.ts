import { describe, expect, it } from 'vitest';
import * as v from 'valibot';
import {
	applyItemChange,
	applyNeedsYouChange,
	availableActions,
	blockedReason,
	BOARD_COLUMNS,
	boardColumn,
	boardItems,
	canComment,
	classifyNeedsYou,
	COMMENT_MAX_LENGTH,
	countPossiblyRunning,
	githubRefLabel,
	groupByColumn,
	groupWorkItems,
	isTerminal,
	itemUrl,
	keyToUrl,
	NEEDS_YOU_CATEGORIES,
	parseReadiness,
	parseWait,
	parseWorkId,
	primaryAction,
	shouldRefresh,
	stateTone,
	SubmitActionSchema,
	TERMINAL_STATES,
	waitLinks,
	type NeedsYou,
	type NeedsYouCategory,
	type WorkItemSummary
} from './work-items.ts';

describe('needs-you classification', () => {
	const NOW = Date.parse('2026-10-14T12:00:00Z');
	const HOUR = 3_600_000;
	const opts = { now: NOW, staleAfterMs: 24 * HOUR };
	const ago = (ms: number) => new Date(NOW - ms).toISOString();
	const cr = (state: string, stateData: unknown = {}, updatedAt: string = ago(30 * HOUR)) => ({
		workflow: 'change_request',
		state,
		stateData,
		updatedAt
	});
	const red = (check: 'ci' | 'review' | 'mergeable') => ({
		checklist: { [check]: { status: 'failed', detail: 'red' } }
	});
	const classify = (item: ReturnType<typeof cr>) => classifyNeedsYou(item, opts);

	it('does not list blocked change requests that are fresh', () => {
		expect(classify(cr('AwaitingChanges', {}, ago(HOUR)))).toBeNull();
		for (const check of ['ci', 'review', 'mergeable'] as const)
			expect(classify(cr('AwaitingChanges', red(check), ago(HOUR)))).toBeNull();
		expect(classify(cr('Opened', red('ci'), ago(HOUR)))).toBeNull();
	});
	it('lists blocked change requests once idle past the threshold', () => {
		expect(classify(cr('AwaitingChanges'))).toEqual({
			category: 'blocked',
			reason: 'Changes requested'
		});
		expect(classify(cr('AwaitingChanges', red('ci')))).toEqual({
			category: 'blocked',
			reason: 'CI: red'
		});
		expect(classify(cr('AwaitingChanges', red('mergeable')))?.category).toBe('blocked');
		expect(classify(cr('Opened', red('review')))?.category).toBe('blocked');
	});
	it('counts the threshold itself as idle, a millisecond earlier as fresh', () => {
		expect(classify(cr('AwaitingChanges', {}, ago(24 * HOUR)))?.category).toBe('blocked');
		expect(classify(cr('AwaitingChanges', {}, ago(24 * HOUR - 1)))).toBeNull();
		expect(
			classifyNeedsYou(cr('AwaitingChanges', {}, ago(2 * HOUR)), { now: NOW, staleAfterMs: HOUR })
				?.category
		).toBe('blocked');
	});
	it('never lists a change request enzarb is working on, however old', () => {
		for (const state of ['Reviewing', 'AddressingComments', 'ResolvingConflicts', 'Merging'])
			for (const check of ['ci', 'review', 'mergeable'] as const)
				expect(classify(cr(state, red(check), ago(100 * HOUR)))).toBeNull();
	});
	it('always lists a parked conflict', () => {
		expect(classify(cr('ConflictParked', {}, ago(1000)))).toEqual({
			category: 'blocked',
			reason: 'Merge conflict needs a decision'
		});
	});
	it('does not list a blocked change request without a usable updatedAt', () => {
		expect(classify({ ...cr('AwaitingChanges'), updatedAt: 'nope' })).toBeNull();
		expect(
			classifyNeedsYou(
				{ workflow: 'change_request', state: 'AwaitingChanges', stateData: {} },
				opts
			)
		).toBeNull();
	});
	it('classifies each category', () => {
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'AwaitingApproval', stateData: {} })
		).toEqual({ category: 'approval', reason: null });
		expect(classify(cr('ReadyToMerge', {}, ago(1000)))?.category).toBe('merge');
		expect(classify(cr('Failed', {}, ago(1000)))?.category).toBe('failed');
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'Failed', stateData: { error: ' boom ' } })
		).toEqual({ category: 'failed', reason: 'boom' });
		expect(classifyNeedsYou({ workflow: 'issue', state: 'Stale', stateData: null })?.category).toBe(
			'failed'
		);
	});
	it('puts a ci_failed wait in Needs you with the failing checks and a Retry', () => {
		const text = 'CI failed: actionlint, scripts';
		const data = {
			wait: { waiting_on: 'human', reason: 'ci_failed', text, url: 'https://ci.example/run/1' }
		};
		expect(classifyNeedsYou(cr('AwaitingChanges', data))).toEqual({
			category: 'failed',
			reason: text,
			text
		});
		expect(availableActions('AwaitingChanges', data)).toContain('retry');
		expect(availableActions('AwaitingChanges', {})).not.toContain('retry');
		expect(primaryAction('failed')).toBe('retry');
		// enzarb is on it: not in the queue, and no retry
		const fixing = { wait: { waiting_on: 'enzarb', reason: 'enzarb_fixing', text: 'x' } };
		expect(classifyNeedsYou(cr('AddressingComments', fixing))).toBeNull();
		expect(availableActions('AddressingComments', fixing)).not.toContain('retry');
	});
	it('labels why a change request is waiting and keeps round_limit in the queue', () => {
		const wait = (waiting_on: string, reason: string, text: string) => ({
			wait: { waiting_on, reason, text }
		});
		const text = 'Automatic fix rounds used up, waiting for you';
		expect(classifyNeedsYou(cr('AwaitingChanges', wait('human', 'round_limit', text)))).toEqual({
			category: 'blocked',
			reason: null,
			text
		});
		// enzarb is working on it: nobody needs to act.
		expect(
			classifyNeedsYou(
				cr(
					'AddressingComments',
					wait('enzarb', 'enzarb_fixing', 'enzarb is fixing its own review findings')
				)
			)
		).toBeNull();
	});
	it('classifies an issue awaiting answers as a question with no action', () => {
		expect(
			classifyNeedsYou({ workflow: 'issue', state: 'AwaitingAnswers', stateData: {} })
		).toEqual({ category: 'question', reason: null });
		expect(primaryAction('question')).toBeNull();
		expect(
			classifyNeedsYou({ workflow: 'change_request', state: 'AwaitingAnswers', stateData: {} })
		).toBeNull();
		expect(classifyNeedsYou({ workflow: 'issue', state: 'Stale', stateData: {} })?.category).toBe(
			'failed'
		);
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
		expect(primaryAction('review')).toBeNull();
		expect(primaryAction('decision')).toBeNull();
	});
	const withWait = (waiting_on: string, reason: string, text = 'T', extra = {}) => ({
		wait: { waiting_on, reason, text, ...extra }
	});
	it('classifies a change request by its wait reason', () => {
		expect(
			classifyNeedsYou(
				cr('AwaitingChanges', withWait('human', 'awaiting_review', 'Waiting for your review'))
			)
		).toEqual({
			category: 'review',
			reason: null,
			text: 'Waiting for your review'
		});
		const unverified = 'enzarb could not verify: connectors tests (missing_tool)';
		expect(
			classifyNeedsYou(cr('AwaitingChanges', withWait('human', 'checks_unverified', unverified)))
		).toEqual({ category: 'review', reason: null, text: unverified });
		expect(
			classifyNeedsYou(
				cr('AwaitingChanges', withWait('human', 'ci_stale', 'CI status looks stale; check GitHub'))
			)
		).toMatchObject({ category: 'blocked', text: 'CI status looks stale; check GitHub' });
		// a reason this UI does not know yet is still the user's turn
		expect(
			classifyNeedsYou(cr('AwaitingChanges', withWait('human', 'from_the_future')))?.category
		).toBe('blocked');
		expect(
			classifyNeedsYou(cr('ReadyToMerge', withWait('human', 'ready_to_merge')))?.category
		).toBe('merge');
		expect(
			classifyNeedsYou(cr('AwaitingChanges', withWait('human', 'decision_needed')))?.category
		).toBe('decision');
		for (const reason of ['conflict_parked', 'round_limit', 'enzarb_failed']) {
			expect(classifyNeedsYou(cr('ConflictParked', withWait('human', reason)))?.category).toBe(
				'blocked'
			);
		}
		for (const [on, reason] of [
			['enzarb', 'reviewing'],
			['enzarb', 'addressing_comments'],
			['enzarb', 'resolving_conflict'],
			['enzarb', 'ci_failed_fixing'],
			['enzarb', 'changes_requested_fixing'],
			['ci', 'ci_pending'],
			['github', 'merging']
		]) {
			// enzarb, CI or GitHub is on it: not the user's turn, even when the state alone would say so
			expect(classifyNeedsYou(cr('AwaitingChanges', withWait(on, reason)))).toBeNull();
		}
	});
	it('falls back to the state when wait is absent, null or unknown', () => {
		for (const wait of [undefined, null, 'x', { waiting_on: 'bogus' }]) {
			expect(classify(cr('AwaitingChanges', { wait }))?.category).toBe('blocked');
		}
		expect(classifyNeedsYou(cr('Merged', withWait('human', 'awaiting_review')))).toBeNull();
	});
	it('a failed change request still offers retry', () => {
		expect(classifyNeedsYou(cr('Failed', withWait('human', 'enzarb_failed')))?.category).toBe(
			'failed'
		);
	});
	it('parseWait reads state_data.wait defensively', () => {
		expect(parseWait(withWait('ci', 'ci_pending', ' Waiting for CI '))).toEqual({
			waitingOn: 'ci',
			reason: 'ci_pending',
			text: 'Waiting for CI',
			url: null
		});
		const url = 'https://github.com/a/b/pull/1#issuecomment-5';
		expect(parseWait(withWait('human', 'decision_needed', 'T', { url }))?.url).toBe(url);
		expect(
			parseWait(withWait('human', 'decision_needed', 'T', { url: 'javascript:alert(1)' }))?.url
		).toBeNull();
		for (const bad of [null, undefined, 5, {}, { wait: null }, { wait: { waiting_on: 'x' } }])
			expect(parseWait(bad)).toBeNull();
	});
	it('waitLinks gives the buttons and their urls', () => {
		const pr = 'https://github.com/acme/web/pull/7';
		const w = (reason: string, extra = {}) => parseWait(withWait('human', reason, 'T', extra));
		expect(waitLinks(w('awaiting_review'), pr)).toEqual([
			{ label: 'Review on GitHub', href: `${pr}/files` }
		]);
		expect(waitLinks(w('checks_unverified'), pr)).toEqual([
			{ label: 'Review on GitHub', href: `${pr}/files` }
		]);
		expect(waitLinks(parseWait(withWait('ci', 'ci_pending')), pr)).toEqual([
			{ label: 'View checks', href: `${pr}/checks` }
		]);
		expect(waitLinks(w('ci_stale'), pr)).toEqual([{ label: 'View checks', href: `${pr}/checks` }]);
		for (const reason of ['enzarb_failed', 'round_limit', 'conflict_parked'])
			expect(waitLinks(w(reason), pr)).toEqual([{ label: 'Open PR', href: pr }]);
		expect(waitLinks(w('ci_failed'), pr)).toEqual([
			{ label: 'View failing check', href: `${pr}/checks` }
		]);
		expect(waitLinks(w('ci_failed', { url: 'https://ci.example/run/1' }), pr)).toEqual([
			{ label: 'View failing check', href: 'https://ci.example/run/1' }
		]);
		const comment = `${pr}#issuecomment-9`;
		expect(waitLinks(w('decision_needed', { url: comment }), pr)).toEqual([
			{ label: 'View comment', href: comment }
		]);
		expect(waitLinks(w('decision_needed'), pr)).toEqual([]);
		expect(waitLinks(w('ready_to_merge'), pr)).toEqual([]);
		expect(waitLinks(parseWait(withWait('enzarb', 'reviewing')), pr)).toEqual([]);
		// no PR url (null, or the issue url derived from the key): no link rather than a wrong one
		expect(waitLinks(w('awaiting_review'), null)).toEqual([]);
		expect(waitLinks(w('awaiting_review'), 'https://github.com/acme/web/issues/7')).toEqual([]);
		expect(waitLinks(null, pr)).toEqual([]);
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
});

describe('board columns', () => {
	const needs = (category: NeedsYou['category']): NeedsYou => ({ category, reason: null });
	const mk = (
		id: number,
		state: string,
		n: NeedsYou | null = null,
		updatedAt = '2026-10-14T10:00:00Z'
	) =>
		({
			id,
			key: `github:o/r#${id}`,
			project: 'p',
			workflow: 'issue',
			state,
			title: `t${id}`,
			url: null,
			updatedAt,
			needs: n
		}) satisfies WorkItemSummary;

	it('runs in process order, left to right', () => {
		expect(BOARD_COLUMNS.map((c) => c.id)).toEqual([
			'plan',
			'progress',
			'blocked',
			'review',
			'done'
		]);
		expect(BOARD_COLUMNS.filter((c) => c.needsYou).map((c) => c.id)).toEqual([
			'plan',
			'blocked',
			'review'
		]);
	});

	it('places every needs-you category', () => {
		const want: Record<NeedsYouCategory, string> = {
			approval: 'plan',
			question: 'plan',
			blocked: 'blocked',
			failed: 'blocked',
			merge: 'review',
			review: 'review',
			decision: 'review'
		};
		expect(Object.keys(want).sort()).toEqual([...NEEDS_YOU_CATEGORIES].sort());
		for (const c of NEEDS_YOU_CATEGORIES)
			expect(boardColumn({ state: 'AwaitingChanges', needs: needs(c) })).toBe(want[c]);
	});

	it('places the rest by whether the state is terminal', () => {
		for (const s of TERMINAL_STATES) expect(boardColumn({ state: s })).toBe('done');
		for (const s of ['Triaging', 'Planning', 'Merging', 'Reviewing', 'Cancelling'])
			expect(boardColumn({ state: s })).toBe('progress');
		// waits on enzarb, CI or a stacked parent stay in progress
		expect(boardColumn({ state: 'AwaitingChanges', needs: null })).toBe('progress');
		expect(boardColumn({ state: 'SomethingNew' })).toBe('progress');
		expect(boardColumn({ state: 'Cancelled' })).toBe('done');
	});

	it('agrees with classifyNeedsYou, including parked conflicts', () => {
		const NOW = Date.parse('2026-10-14T12:00:00Z');
		const classify = (workflow: string, state: string, stateData: unknown = {}) =>
			classifyNeedsYou(
				{ workflow, state, stateData, updatedAt: '2026-10-14T11:00:00Z' },
				{ now: NOW, staleAfterMs: 24 * 3_600_000 }
			);
		const column = (workflow: string, state: string, stateData?: unknown) =>
			boardColumn({ state, needs: classify(workflow, state, stateData) });
		expect(column('issue', 'AwaitingApproval')).toBe('plan');
		expect(column('issue', 'AwaitingAnswers')).toBe('plan');
		expect(column('issue', 'Failed')).toBe('blocked');
		expect(column('change_request', 'ConflictParked')).toBe('blocked');
		expect(column('change_request', 'ReadyToMerge')).toBe('review');
		expect(column('change_request', 'Reviewing')).toBe('progress');
		expect(column('issue', 'Done')).toBe('done');
		const wait = (reason: string, waiting_on: string) => ({
			wait: { waiting_on, reason, text: 't' }
		});
		expect(column('change_request', 'AwaitingChanges', wait('conflict_parked', 'human'))).toBe(
			'blocked'
		);
		expect(column('change_request', 'AwaitingChanges', wait('ci_pending', 'ci'))).toBe('progress');
	});

	it('groups items per column in input order, with every column present', () => {
		const items = [
			mk(1, 'Planning'),
			mk(2, 'AwaitingApproval', needs('approval')),
			mk(3, 'Done'),
			mk(4, 'Planning')
		];
		const g = groupByColumn(items);
		expect(g.progress.map((i) => i.id)).toEqual([1, 4]);
		expect(g.plan.map((i) => i.id)).toEqual([2]);
		expect(g.done.map((i) => i.id)).toEqual([3]);
		expect(g.blocked).toEqual([]);
		expect(g.review).toEqual([]);
	});

	it('merges the queue and the list: filtered, deduplicated, queue wins, newest first', () => {
		const queue = [
			mk(1, 'AwaitingApproval', needs('approval'), '2026-10-14T09:00:00Z'),
			{ ...mk(2, 'Failed', needs('failed')), project: 'other' }
		];
		const list = [
			mk(1, 'AwaitingApproval', needs('approval'), '2026-10-14T08:00:00Z'),
			mk(3, 'Planning', null, '2026-10-14T11:00:00Z'),
			// the list copy still says "waiting"; the queue is what moves it
			mk(4, 'AwaitingApproval', needs('approval'))
		];
		expect(boardItems(queue, list).map((i) => i.id)).toEqual([3, 2, 1]);
		expect(boardItems(queue, list, 'p').map((i) => i.id)).toEqual([3, 1]);
		expect(boardItems(queue, list)[2].updatedAt).toBe('2026-10-14T09:00:00Z');
		expect(boardItems([], [])).toEqual([]);
	});
});

describe('comment', () => {
	it('is offered on an issue plan or questions only', () => {
		expect(canComment('issue', 'AwaitingApproval')).toBe(true);
		expect(canComment('issue', 'AwaitingAnswers')).toBe(true);
		expect(canComment('issue', 'Planning')).toBe(false);
		expect(canComment('change_request', 'AwaitingApproval')).toBe(false);
		expect(availableActions('AwaitingApproval')).not.toContain('comment');
	});

	it('needs text and caps its length', () => {
		const ok = (x: unknown) => v.safeParse(SubmitActionSchema, x).success;
		expect(ok({ id: 1, action: 'approve' })).toBe(true);
		expect(ok({ id: 1, action: 'merge', data: { deleteBranch: true } })).toBe(true);
		expect(ok({ id: 1, action: 'comment', data: { body: 'use CSS vars' } })).toBe(true);
		expect(ok({ id: 1, action: 'comment' })).toBe(false);
		expect(ok({ id: 1, action: 'comment', data: { body: '   ' } })).toBe(false);
		expect(
			ok({ id: 1, action: 'comment', data: { body: 'x'.repeat(COMMENT_MAX_LENGTH + 1) } })
		).toBe(false);
		expect(ok({ id: 1, action: 'nuke' })).toBe(false);
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

describe('githubRefLabel', () => {
	it('labels issue and pull URLs with owner/repo#n', () => {
		expect(githubRefLabel('https://github.com/acme/web/issues/7')).toBe('acme/web#7');
		expect(githubRefLabel('https://github.com/acme/web/pull/12')).toBe('acme/web#12');
	});
	it('drops the repo for single-repo projects', () => {
		expect(githubRefLabel('https://github.com/acme/web/issues/7', true)).toBe('#7');
		expect(githubRefLabel('https://github.com/acme/web/issues/7', false)).toBe('acme/web#7');
	});
	it('returns null for other or missing URLs', () => {
		expect(githubRefLabel('https://example.com/acme/web/issues/7')).toBeNull();
		expect(githubRefLabel('https://github.com/acme/web')).toBeNull();
		expect(githubRefLabel('nonsense')).toBeNull();
		expect(githubRefLabel(null)).toBeNull();
	});
});

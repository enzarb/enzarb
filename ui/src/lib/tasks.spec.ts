import * as v from 'valibot';
import { describe, expect, it } from 'vitest';
import { appendEvents, isError, matches } from './session.ts';
import {
	SubmitSchema,
	applyRow,
	commentKind,
	groupByProject,
	originLabel,
	originUrl,
	placeCards,
	findingTarget,
	severityClass,
	severityRank
} from './tasks.ts';
import { row } from './test/task-row.ts';

describe('placeCards', () => {
	it('keeps the board order and puts unknown columns in Other', () => {
		const board = {
			columns: [
				{ id: 'doing', title: 'Doing' },
				{ id: 'review', title: 'Review' }
			]
		};
		const out = placeCards(board, [
			row({ id: 1, column: 'review' }),
			row({ id: 2, column: 'doing' }),
			row({ id: 3, column: 'gone' }),
			row({ id: 4, column: null })
		]);
		expect(out.map((c) => [c.column.title, c.tasks.map((t) => t.id)])).toEqual([
			['Doing', [2]],
			['Review', [1]],
			['Other', [3, 4]]
		]);
	});
});

describe('helpers', () => {
	it('groups by project in name order', () => {
		const g = groupByProject([row({ id: 1, project: 'b' }), row({ id: 2, project: 'a' })]);
		expect(g.map(([p]) => p)).toEqual(['a', 'b']);
	});

	it('replaces, adds and drops rows', () => {
		const rows = [row({ id: 1 }), row({ id: 2 })];
		expect(applyRow(rows, 2, row({ id: 2, title: 'x' }))[1].title).toBe('x');
		expect(applyRow(rows, 3, row({ id: 3 })).map((r) => r.id)).toEqual([3, 1, 2]);
		expect(applyRow(rows, 1, null).map((r) => r.id)).toEqual([2]);
		expect(applyRow(rows, 1, row({ id: 1 }), (t) => t.needsYou).map((r) => r.id)).toEqual([2]);
	});

	it('steers while running and comments otherwise', () => {
		expect(commentKind('running')).toBe('steer');
		expect(commentKind('waiting')).toBe('comment');
	});

	it('builds origin badges and source links', () => {
		const o = {
			layer: 'library',
			repo: 'enzarb/workflows',
			sha: 'abcdef1234',
			path: 'phases/plan.yaml'
		};
		expect(originLabel(o)).toBe('library@abcdef1');
		expect(originUrl(o)).toBe(
			'https://github.com/enzarb/workflows/blob/abcdef1234/phases/plan.yaml'
		);
		expect(originUrl({ layer: 'project' })).toBeNull();
		expect(originLabel(undefined)).toBe('unknown');
	});

	it('validates submissions', () => {
		const ok = (x: unknown) => v.safeParse(SubmitSchema, x).success;
		expect(ok({ id: 1, kind: 'command', data: { command: 'approve' } })).toBe(true);
		expect(ok({ id: 1, kind: 'command', data: { command: 'a b' } })).toBe(false);
		expect(ok({ id: 1, kind: 'steer', data: { body: 'go' } })).toBe(true);
		expect(ok({ id: 1, kind: 'comment', data: { body: '  ' } })).toBe(false);
		expect(ok({ id: 1, kind: 'cancel' })).toBe(true);
		expect(ok({ id: 1, kind: 'merge' })).toBe(false);
	});
});

describe('session helpers', () => {
	const e = (seq: number, kind: string, payload: unknown) => ({
		seq,
		at: '2026-10-16T00:00:00.000Z',
		kind,
		payload,
		truncated: false
	});

	it('appends without duplicates', () => {
		expect(
			appendEvents([e(1, 'user', {})], [e(1, 'user', {}), e(2, 'assistant', {})]).map((x) => x.seq)
		).toEqual([1, 2]);
	});

	it('finds errors and matches text', () => {
		expect(isError(e(1, 'tool_result', { is_error: true }))).toBe(true);
		expect(isError(e(1, 'stderr', { text: 'x' }))).toBe(true);
		expect(isError(e(1, 'assistant', { text: 'x' }))).toBe(false);
		expect(matches(e(1, 'assistant', { text: 'Hello World' }), 'world')).toBe(true);
		expect(matches(e(1, 'assistant', { text: 'Hello' }), 'nope')).toBe(false);
	});
});

describe('findings helpers', () => {
	it('ranks severities and tolerates unknown ones', () => {
		expect(severityRank('critical')).toBeGreaterThan(severityRank('warning'));
		expect(severityRank('warning')).toBeGreaterThan(severityRank('info'));
		expect(severityRank('fatal')).toBe(0);
		expect(severityClass('critical')).toBe('critical');
		expect(severityClass('fatal')).toBe('unknown');
	});

	it('targets the task, or the project and check', () => {
		expect(findingTarget({ taskId: 4, project: 'web', checkId: 'c' })).toEqual({
			href: '/task/4',
			label: 'Task #4'
		});
		expect(findingTarget({ taskId: null, project: 'web', checkId: 'c' })).toEqual({
			href: null,
			label: 'web · c'
		});
	});
});

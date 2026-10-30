import { describe, expect, it } from 'vitest';
import { decisionHeadline, repoKeyFromUrl, summarizeEvent } from './events.ts';

describe('repoKeyFromUrl', () => {
	it('lowercases owner and repo and drops .git', () => {
		expect(repoKeyFromUrl('https://github.com/EnZarb/UI.git')).toBe('enzarb/ui');
		expect(repoKeyFromUrl('https://github.com/enzarb/enzarb/')).toBe('enzarb/enzarb');
	});
	it('rejects what is not owner/repo', () => {
		expect(repoKeyFromUrl('https://github.com/enzarb')).toBeNull();
		expect(repoKeyFromUrl('not a url')).toBeNull();
	});
});

describe('summarizeEvent', () => {
	it('prefers the issue or pull request title', () => {
		expect(summarizeEvent({ issue: { title: 'Broken build' }, ref: 'x' })).toBe('Broken build');
		expect(summarizeEvent({ pull_request: { title: 'Add thing' } })).toBe('Add thing');
	});
	it('uses the first line of a commit message and a ref as fallbacks', () => {
		expect(summarizeEvent({ head_commit: { message: 'fix: a\n\nbody' } })).toBe('fix: a');
		expect(summarizeEvent({ ref: 'refs/heads/main' })).toBe('refs/heads/main');
	});
	it('truncates long lines and returns null for nothing useful', () => {
		expect(summarizeEvent({ issue: { title: 'x'.repeat(300) } })!.length).toBe(160);
		expect(summarizeEvent({ zen: 'Keep it logically awesome.' })).toBeNull();
		expect(summarizeEvent(null)).toBeNull();
	});
});

describe('decisionHeadline', () => {
	const base = {
		eventId: 1,
		project: 'p',
		sha: 'abc1234',
		started: [],
		reasons: [],
		errors: [],
		decidedAt: '2026-10-30T00:00:00.000Z'
	};
	it('names the started workflows', () => {
		expect(decisionHeadline({ ...base, outcome: 'started', started: ['a', 'b'] })).toEqual({
			text: 'Started a, b',
			tone: 'ok'
		});
	});
	it('says when no rule matched, when routing failed and when it was skipped', () => {
		expect(decisionHeadline({ ...base, outcome: 'no_match' }).text).toBe('No rule matched');
		expect(decisionHeadline({ ...base, outcome: 'error' }).tone).toBe('error');
		expect(decisionHeadline({ ...base, outcome: 'skipped' }).tone).toBe('muted');
	});
});

import { describe, expect, it } from 'vitest';
import {
	approvalBody,
	checksText,
	mergeBlock,
	mergeTitle,
	summarizeChecks,
	type CheckRun,
	type PullState
} from './pull-review.ts';

const check = (name: string, status: string, conclusion: string | null): CheckRun => ({
	name,
	status,
	conclusion,
	url: ''
});
const pull: PullState = {
	number: 5,
	title: 'Fix it',
	url: '',
	draft: false,
	headSha: 'abc',
	mergeable: true,
	mergeableState: 'clean'
};
const green = summarizeChecks([check('build', 'completed', 'success')]);

describe('checks', () => {
	it('counts passed, failed and pending', () => {
		const s = summarizeChecks([
			check('a', 'completed', 'success'),
			check('b', 'completed', 'skipped'),
			check('lint', 'completed', 'failure'),
			check('c', 'completed', 'cancelled'),
			check('e2e', 'in_progress', null)
		]);
		expect(s).toEqual({ passed: 2, failed: ['lint', 'c'], pending: ['e2e'] });
		expect(checksText(s)).toBe('2 passed, 2 failed (lint, c), 1 pending');
		expect(checksText(summarizeChecks([]))).toBe('No checks ran');
	});
});

describe('mergeBlock', () => {
	it('allows a clean pull request with green checks', () => {
		expect(mergeBlock(pull, green)).toBeNull();
		expect(mergeBlock(pull, summarizeChecks([]))).toBeNull();
	});
	it('refuses red checks, naming them', () => {
		const red = summarizeChecks([check('lint', 'completed', 'failure')]);
		expect(mergeBlock(pull, red)).toBe('Checks are failing: lint.');
	});
	it('refuses conflicts', () => {
		expect(mergeBlock({ ...pull, mergeable: false, mergeableState: 'dirty' }, green)).toMatch(
			/conflicts/
		);
	});
	it('refuses pending checks and an unknown mergeability', () => {
		const pend = summarizeChecks([check('e2e', 'queued', null)]);
		expect(mergeBlock(pull, pend)).toBe('Checks are still running: e2e.');
		expect(mergeBlock({ ...pull, mergeable: null }, green)).toMatch(/still working out/);
	});
	it('refuses drafts', () => {
		expect(mergeBlock({ ...pull, draft: true }, green)).toMatch(/draft/);
	});
});

describe('wording', () => {
	it('names who approved and keeps the PR number in the title', () => {
		expect(approvalBody('octo')).toBe('Approved by @octo in enzarb');
		expect(mergeTitle('Fix it', 5)).toBe('Fix it (#5)');
	});
});

import { describe, expect, it } from 'vitest';
import { repoContext, runLabel, subjectLabel } from './github-links.ts';

describe('subjectLabel', () => {
	const ctx = repoContext(['enzarb/enzarb', 'enzarb/workflows', 'other/tools']);
	it('drops the owner when it is the Project owner', () => {
		expect(subjectLabel('enzarb/enzarb', 43, ctx)).toBe('enzarb#43');
	});
	it('keeps the owner when it differs', () => {
		expect(subjectLabel('other/tools', 7, ctx)).toBe('other/tools#7');
	});
	it('keeps the owner when the name is ambiguous', () => {
		const c = repoContext(['enzarb/app', 'enzarb/web', 'x/app']);
		expect(subjectLabel('enzarb/app', 1, c)).toBe('enzarb/app#1');
	});
	it('shows the full name without context and omits a missing number', () => {
		expect(subjectLabel('a/b', 2)).toBe('a/b#2');
		expect(subjectLabel('enzarb/enzarb', null, ctx)).toBe('enzarb');
	});
});

describe('runLabel', () => {
	it('is the workflow and the first 8 characters of the id', () => {
		expect(runLabel('review-pull-requests', '1a2b3c4d-0000-4000-8000-000000000000')).toBe(
			'review-pull-requests #1a2b3c4d'
		);
	});
});

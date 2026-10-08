import { describe, expect, it } from 'vitest';
import { availableActions, isTerminal, itemUrl, keyToUrl, stateTone } from './work-items.ts';

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

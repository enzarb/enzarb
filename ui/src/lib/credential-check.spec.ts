import { describe, expect, it } from 'vitest';
import {
	advanceCheck,
	buildCredentialSteps,
	describeCredentialError,
	failCheck,
	isUnimplemented,
	startCheck
} from './credential-check.ts';

const states = (c: Parameters<typeof buildCredentialSteps>[0]) =>
	buildCredentialSteps(c).map((s) => s.state);

describe('credential check checklist', () => {
	it('starts with the first step in progress', () => {
		expect(states(startCheck)).toEqual(['in-progress', 'pending', 'pending']);
	});

	it('advances step by step and finishes when answered', () => {
		expect(states(advanceCheck(advanceCheck(startCheck)))).toEqual(['done', 'done', 'in-progress']);
		expect(states({ phase: 'answered', configured: false })).toEqual(['done', 'done', 'done']);
	});

	it('marks the running step failed', () => {
		const failed = failCheck(advanceCheck(startCheck), new Error('socket closed'));
		expect(failed).toMatchObject({ phase: 'unavailable', step: 1, reason: 'socket closed' });
		expect(states(failed)).toEqual(['done', 'failed', 'pending']);
	});
});

describe('unimplemented handling', () => {
	const remote = (code: string) =>
		Object.assign(new Error('remote'), { code: 'remote', details: { remote: { code } } });

	it('detects a remote unimplemented error', () => {
		expect(isUnimplemented(remote('unimplemented'))).toBe(true);
		expect(isUnimplemented(remote('other'))).toBe(false);
		expect(isUnimplemented(new Error('x'))).toBe(false);
		expect(isUnimplemented(null)).toBe(false);
	});

	it('describes it in plain words', () => {
		expect(describeCredentialError(remote('unimplemented'))).toMatch(/does not report/);
		expect(describeCredentialError('weird')).toBe('weird');
	});
});

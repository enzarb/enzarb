import { describe, expect, it } from 'vitest';
import {
	buildSteps,
	completeStep,
	describeClose,
	failCurrent,
	initialProgress
} from './terminal-steps.ts';

const states = (p: Parameters<typeof buildSteps>[0]) => buildSteps(p).map((s) => s.state);

describe('terminal steps', () => {
	it('starts with the token step in progress', () => {
		expect(states(initialProgress)).toEqual(['in-progress', 'pending', 'pending', 'pending']);
	});

	it('advances in order to all done', () => {
		let p = completeStep(initialProgress, 'token');
		expect(states(p)).toEqual(['done', 'in-progress', 'pending', 'pending']);
		p = completeStep(completeStep(completeStep(p, 'connecting'), 'handshake'), 'shell');
		expect(states(p)).toEqual(['done', 'done', 'done', 'done']);
	});

	it('fails the running step and freezes progress', () => {
		let p = completeStep(initialProgress, 'token');
		p = failCurrent(p, 'refused');
		expect(states(p)).toEqual(['done', 'failed', 'pending', 'pending']);
		expect(buildSteps(p)[1].message).toBe('refused');
		expect(completeStep(p, 'connecting')).toBe(p);
		expect(failCurrent(p, 'again')).toBe(p);
	});
});

describe('describeClose', () => {
	it('explains exit, signal and unknown reasons', () => {
		expect(describeClose('exit:0')).toBe('Shell exited');
		expect(describeClose('exit:2')).toBe('Shell exited with code 2');
		expect(describeClose('signal:9')).toBe('Shell killed by signal 9');
		expect(describeClose('going away')).toBe('Session closed: going away');
		expect(describeClose(undefined)).toBe('Session closed');
	});
});

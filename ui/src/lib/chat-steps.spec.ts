import { describe, expect, it } from 'vitest';
import {
	CHAT_STEPS,
	buildChatSteps,
	completeChatStep,
	failChatStep,
	initialChatProgress,
	type ChatProgress
} from './chat-steps.ts';

const states = (p: ChatProgress) => buildChatSteps(p).map((s) => s.state);

describe('chat steps', () => {
	it('has the six steps in order', () => {
		expect(CHAT_STEPS.map((s) => s.label)).toEqual([
			'Session token',
			'Connecting to sandbox',
			'Protocol handshake',
			'Start agent',
			'Initialize',
			'Chat session'
		]);
	});

	it('starts with the first step active', () => {
		expect(states(initialChatProgress)).toEqual([
			'active',
			'pending',
			'pending',
			'pending',
			'pending',
			'pending'
		]);
	});

	it('advances and finishes', () => {
		let p = initialChatProgress;
		for (const s of CHAT_STEPS) p = completeChatStep(p, s.id);
		expect(states(p)).toEqual(Array(6).fill('done'));
	});

	it('completing an earlier step again does not go back', () => {
		const p = completeChatStep(completeChatStep(initialChatProgress, 'token'), 'connect');
		expect(completeChatStep(p, 'token')).toBe(p);
	});

	it('an open_error fails "Start agent"', () => {
		let p = initialChatProgress;
		for (const id of ['token', 'connect', 'handshake'] as const) p = completeChatStep(p, id);
		p = failChatStep(p, 'No credential');
		const steps = buildChatSteps(p);
		expect(steps[3]).toMatchObject({ id: 'agent', state: 'error', message: 'No credential' });
		expect(steps[4].state).toBe('pending');
	});

	it('earlier failures fail their own step', () => {
		const p = failChatStep(completeChatStep(initialChatProgress, 'token'), 'refused');
		expect(buildChatSteps(p)[1]).toMatchObject({ state: 'error', message: 'refused' });
	});

	it('shows a static "Starting agent…" label while the agent step is active', () => {
		let p = initialChatProgress;
		for (const id of ['token', 'connect', 'handshake'] as const) p = completeChatStep(p, id);
		expect(buildChatSteps(p)[3]).toMatchObject({ state: 'active', label: 'Starting agent…' });
		expect(buildChatSteps(completeChatStep(p, 'agent'))[3].label).toBe('Start agent');
	});
});

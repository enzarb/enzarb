import { describe, expect, it } from 'vitest';
import { buildChecklist, overallState } from './conditions.ts';
import type { Condition } from './project-types.ts';

const c = (type: string, status: string, reason = 'X', message = ''): Condition => ({
	type,
	status,
	reason,
	message
});
const states = (conds: Condition[]) => buildChecklist(conds).map((s) => s.state);

describe('buildChecklist', () => {
	it('always lists the four steps in boot order', () => {
		expect(buildChecklist([]).map((s) => s.type)).toEqual([
			'TenantReady',
			'NamespaceReady',
			'SandboxReady',
			'Ready'
		]);
	});
	it('is all pending before the operator reports anything', () => {
		expect(states([])).toEqual(['pending', 'pending', 'pending', 'pending']);
		expect(states(undefined as never)).toEqual(['pending', 'pending', 'pending', 'pending']);
	});
	it('orders by boot order regardless of condition order', () => {
		expect(
			states([c('Ready', 'False'), c('TenantReady', 'True'), c('NamespaceReady', 'True')])
		).toEqual(['done', 'done', 'in-progress', 'pending']);
	});
	it('marks the first unfinished step in-progress and carries its message', () => {
		const steps = buildChecklist([
			c('TenantReady', 'True'),
			c('NamespaceReady', 'False', 'Creating', 'waiting for namespace')
		]);
		expect(steps.map((s) => s.state)).toEqual(['done', 'in-progress', 'pending', 'pending']);
		expect(steps[1].message).toBe('waiting for namespace');
	});
	it('marks failure reasons as failed and later steps pending', () => {
		const steps = buildChecklist([
			c('TenantReady', 'True'),
			c('NamespaceReady', 'True'),
			c('SandboxReady', 'False', 'SandboxFailed', 'image pull backoff')
		]);
		expect(steps.map((s) => s.state)).toEqual(['done', 'done', 'failed', 'pending']);
		expect(steps[2].message).toBe('image pull backoff');
	});
	it('treats Unknown as in-progress', () => {
		expect(states([c('TenantReady', 'Unknown')])[0]).toBe('in-progress');
	});
	it('is all done when Ready', () => {
		const all = ['TenantReady', 'NamespaceReady', 'SandboxReady', 'Ready'].map((t) => c(t, 'True'));
		expect(states(all)).toEqual(['done', 'done', 'done', 'done']);
		expect(overallState(all)).toBe('done');
	});
});

describe('overallState', () => {
	it('summarizes pending, in-progress and failed', () => {
		expect(overallState([])).toBe('pending');
		expect(overallState([c('TenantReady', 'True')])).toBe('in-progress');
		expect(overallState([c('TenantReady', 'False', 'TenantFailed')])).toBe('failed');
	});
});

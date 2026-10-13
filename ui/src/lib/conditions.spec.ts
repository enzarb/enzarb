import { describe, expect, it } from 'vitest';
import {
	buildChecklist,
	buildSandboxChangeSteps,
	buildStorageResizeStep,
	isTerminalFailure,
	overallState
} from './conditions.ts';
import type { Condition } from './project-types.ts';

const c = (type: string, status: string, reason = 'X', message = ''): Condition => ({
	type,
	status,
	reason,
	message
});
const states = (conds: Condition[]) => buildChecklist(conds).map((s) => s.state);

describe('buildChecklist', () => {
	it('always lists the three steps in boot order', () => {
		expect(buildChecklist([]).map((s) => s.type)).toEqual([
			'ResourcesReady',
			'SandboxReady',
			'Ready'
		]);
	});
	it('is all pending before the operator reports anything', () => {
		expect(states([])).toEqual(['pending', 'pending', 'pending']);
		expect(states(undefined as never)).toEqual(['pending', 'pending', 'pending']);
	});
	it('orders by boot order regardless of condition order', () => {
		expect(states([c('Ready', 'False'), c('ResourcesReady', 'True')])).toEqual([
			'done',
			'in-progress',
			'pending'
		]);
	});
	it('marks the first unfinished step in-progress and carries its message', () => {
		const steps = buildChecklist([
			c('ResourcesReady', 'True'),
			c('SandboxReady', 'False', 'Creating', 'waiting for pod')
		]);
		expect(steps.map((s) => s.state)).toEqual(['done', 'in-progress', 'pending']);
		expect(steps[1].message).toBe('waiting for pod');
	});
	it('marks failure reasons as failed and later steps pending', () => {
		const steps = buildChecklist([
			c('ResourcesReady', 'True'),
			c('SandboxReady', 'False', 'SandboxFailed', 'image pull backoff')
		]);
		expect(steps.map((s) => s.state)).toEqual(['done', 'failed', 'pending']);
		expect(steps[1].message).toBe('image pull backoff');
	});
	it('treats Unknown as in-progress', () => {
		expect(states([c('ResourcesReady', 'Unknown')])[0]).toBe('in-progress');
	});
	it('shows InvalidSpec on ResourcesReady as a failed step', () => {
		const conds = [c('ResourcesReady', 'False', 'InvalidSpec', 'bad repo url')];
		const steps = buildChecklist(conds);
		expect(steps.map((s) => s.state)).toEqual(['failed', 'pending', 'pending']);
		expect(steps[0].message).toBe('bad repo url');
		expect(overallState(conds)).toBe('failed');
	});
	it('treats Terminating as in progress', () => {
		const conds = [c('ResourcesReady', 'False', 'Terminating', 'previous sandbox is terminating')];
		expect(states(conds)).toEqual(['in-progress', 'pending', 'pending']);
		expect(overallState(conds)).toBe('in-progress');
		expect(isTerminalFailure(conds)).toBe(false);
	});
	it('is all done when Ready', () => {
		const all = ['ResourcesReady', 'SandboxReady', 'Ready'].map((t) => c(t, 'True'));
		expect(states(all)).toEqual(['done', 'done', 'done']);
		expect(overallState(all)).toBe('done');
	});
	it('ignores unknown condition types from an old-style status', () => {
		const old = [
			c('TenantReady', 'True'),
			c('NamespaceReady', 'False', 'NamespaceFailed', 'old'),
			c('SandboxReady', 'True'),
			c('Ready', 'True')
		];
		expect(states(old)).toEqual(['in-progress', 'done', 'done']);
		expect(overallState(old)).toBe('in-progress');
		expect(isTerminalFailure(old)).toBe(false);
		expect(states([c('TenantReady', 'True'), c('NamespaceReady', 'True')])).toEqual([
			'in-progress',
			'pending',
			'pending'
		]);
	});
});

describe('overallState', () => {
	it('summarizes pending, in-progress and failed', () => {
		expect(overallState([])).toBe('pending');
		expect(overallState([c('ResourcesReady', 'True')])).toBe('in-progress');
		expect(overallState([c('SandboxReady', 'False', 'SandboxFailed')])).toBe('failed');
	});
});

describe('isTerminalFailure', () => {
	it('is true for failed steps, InvalidSpec and NotOwnedError, false while progressing or ready', () => {
		expect(isTerminalFailure([c('SandboxReady', 'False', 'SandboxFailed')])).toBe(true);
		expect(isTerminalFailure([c('ResourcesReady', 'False', 'InvalidSpec')])).toBe(true);
		expect(isTerminalFailure([c('ResourcesReady', 'False', 'NotOwnedError')])).toBe(true);
		expect(overallState([c('ResourcesReady', 'False', 'NotOwnedError')])).toBe('failed');
		expect(isTerminalFailure([c('ResourcesReady', 'True')])).toBe(false);
		expect(isTerminalFailure([])).toBe(false);
		expect(isTerminalFailure(undefined)).toBe(false);
	});
});

describe('buildSandboxChangeSteps', () => {
	const src = (
		conditions: Condition[],
		over: Partial<Parameters<typeof buildSandboxChangeSteps>[0]> = {}
	) => ({ conditions, pendingChange: null, generation: 2, observedGeneration: 2, ...over });
	const st = (s: ReturnType<typeof buildSandboxChangeSteps>) => s.map((x) => x.state);
	const pending = { generation: 1, since: '2026-01-01T00:00:00Z', reason: 'cpu' };

	it('saving: only the first step is in progress', () => {
		expect(st(buildSandboxChangeSteps(src([]), true))).toEqual([
			'in-progress',
			'pending',
			'pending',
			'pending'
		]);
	});
	it('queued: pendingChange set', () => {
		const steps = buildSandboxChangeSteps(
			src([c('SandboxChangePending', 'True'), c('Ready', 'True')], { pendingChange: pending }),
			false
		);
		expect(st(steps)).toEqual(['done', 'in-progress', 'pending', 'pending']);
		expect(steps[1].label).toMatch(/waiting for running jobs/);
	});
	it('queued: condition alone is enough', () => {
		expect(st(buildSandboxChangeSteps(src([c('SandboxChangePending', 'True')]), false))[1]).toBe(
			'in-progress'
		);
	});
	it('applying: nothing pending but the operator has not caught up', () => {
		const steps = buildSandboxChangeSteps(
			src([c('SandboxChangePending', 'False'), c('Ready', 'True')], { observedGeneration: 1 }),
			false
		);
		expect(st(steps)).toEqual(['done', 'done', 'in-progress', 'pending']);
	});
	it('applying: SandboxReady not yet True', () => {
		const steps = buildSandboxChangeSteps(
			src([c('SandboxReady', 'False', 'Progressing'), c('Ready', 'False', 'Progressing')]),
			false
		);
		expect(st(steps)).toEqual(['done', 'done', 'in-progress', 'pending']);
	});
	it('ready: Ready True and generation observed', () => {
		const steps = buildSandboxChangeSteps(
			src([c('SandboxReady', 'True'), c('Ready', 'True')]),
			false
		);
		expect(st(steps)).toEqual(['done', 'done', 'done', 'done']);
	});
	it('failed: SandboxReady failure carries its message', () => {
		const steps = buildSandboxChangeSteps(
			src([c('SandboxReady', 'False', 'SandboxFailed', 'pod unschedulable')]),
			false
		);
		expect(st(steps)).toEqual(['done', 'done', 'failed', 'pending']);
		expect(steps[2].message).toBe('pod unschedulable');
	});
	it('failed: queue failure', () => {
		const steps = buildSandboxChangeSteps(
			src([c('SandboxChangePending', 'False', 'DrainFailed', 'stuck')]),
			false
		);
		expect(st(steps)).toEqual(['done', 'failed', 'pending', 'pending']);
		expect(steps[1].message).toBe('stuck');
	});
});

describe('buildStorageResizeStep', () => {
	it('tracks WorkspaceResizing and WorkspaceResizeFailed on ResourcesReady', () => {
		expect(buildStorageResizeStep([c('ResourcesReady', 'False', 'WorkspaceResizing')])?.state).toBe(
			'in-progress'
		);
		const failed = buildStorageResizeStep([
			c('ResourcesReady', 'False', 'WorkspaceResizeFailed', 'quota')
		]);
		expect(failed).toMatchObject({ state: 'failed', message: 'quota' });
		expect(buildStorageResizeStep([c('ResourcesReady', 'True')])).toBeNull();
		expect(buildStorageResizeStep(undefined)).toBeNull();
	});
});

import { describe, expect, it } from 'vitest';
import { pendingPlan } from './plan.ts';
import type { NodeRun } from './runs.ts';

const node = (name: string, status: string, outputs: unknown = null): NodeRun =>
	({ node: name, visit: 1, status, outputs }) as unknown as NodeRun;

describe('pendingPlan', () => {
	it('finds the plan before the waiting node, whatever the nodes are called', () => {
		const p = pendingPlan({
			status: 'waiting',
			nodeRuns: [
				node('think', 'done', { result: { plan: 'old' } }),
				node('think', 'done', { result: { plan: '## New' } }),
				node('say', 'done'),
				node('gate', 'waiting')
			]
		});
		expect(p).toEqual({ plan: '## New', node: 'think', waitNode: 'gate' });
	});

	it('is null unless the run waits, a node waits, and an earlier result has a plan', () => {
		const withPlan = [node('a', 'done', { result: { plan: 'p' } }), node('w', 'waiting')];
		expect(pendingPlan({ status: 'running', nodeRuns: withPlan })).toBeNull();
		expect(pendingPlan({ status: 'waiting', nodeRuns: [node('a', 'done')] })).toBeNull();
		expect(
			pendingPlan({ status: 'waiting', nodeRuns: [node('a', 'done', { result: { plan: 'p' } })] })
		).toBeNull();
		expect(
			pendingPlan({
				status: 'waiting',
				nodeRuns: [node('w', 'waiting'), node('a', 'done', { result: { plan: 'after' } })]
			})
		).toBeNull();
		expect(
			pendingPlan({
				status: 'waiting',
				nodeRuns: [node('a', 'done', { result: { plan: '  ' } }), node('w', 'waiting')]
			})
		).toBeNull();
		expect(
			pendingPlan({
				status: 'waiting',
				nodeRuns: [node('a', 'done', { result: { decision: 'go' } }), node('w', 'waiting')]
			})
		).toBeNull();
	});
});

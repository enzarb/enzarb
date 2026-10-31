import type { NodeRun, RunDetail } from './runs.ts';

/** A plan awaiting approval: the run is waiting and an earlier node's result has a `plan`. */
export interface PendingPlan {
	plan: string;
	/** The node that wrote it. */
	node: string;
	/** The wait node the run is parked at. */
	waitNode: string;
}

const planOfNode = (n: NodeRun): string | null => {
	const result = (n.outputs as { result?: unknown } | null)?.result;
	const plan =
		result && typeof result === 'object' ? (result as { plan?: unknown }).plan : undefined;
	return typeof plan === 'string' && plan.trim() ? plan : null;
};

/**
 * Generic: no workflow or node names. The run waits, a node is waiting, and the newest earlier node
 * run with a `plan` in its result is the plan being approved.
 */
export function pendingPlan(run: Pick<RunDetail, 'status' | 'nodeRuns'>): PendingPlan | null {
	if (run.status !== 'waiting') return null;
	const runs = run.nodeRuns;
	let w = -1;
	for (let i = runs.length - 1; i >= 0; i--)
		if (runs[i]!.status === 'waiting') {
			w = i;
			break;
		}
	if (w < 0) return null;
	for (let i = w - 1; i >= 0; i--) {
		const plan = planOfNode(runs[i]!);
		if (plan) return { plan, node: runs[i]!.node, waitNode: runs[w]!.node };
	}
	return null;
}

import type { NodeDoc, WorkflowDoc } from '#lib/workflows-types.ts';

/** Known GitHub event/action pairs for the event picker; free text is still accepted. */
export const KNOWN_EVENTS: { value: string; label: string }[] = [
	['issues.opened', 'an issue is opened'],
	['issues.labeled', 'an issue is labeled'],
	['issues.assigned', 'an issue is assigned'],
	['issues.closed', 'an issue is closed'],
	['issue_comment.created', 'a comment is added to an issue'],
	['pull_request.opened', 'a pull request is opened'],
	['pull_request.labeled', 'a pull request is labeled'],
	['pull_request.synchronize', 'a pull request is updated'],
	['pull_request.closed', 'a pull request is closed'],
	['pull_request_review.submitted', 'a pull request review is submitted'],
	['push', 'code is pushed'],
	['workflow_run.completed', 'a CI run completes']
].map(([value, label]) => ({ value, label }));

/** `issues.labeled` as "an issue is labeled"; unknown values are shown as written. */
export function eventLabel(on: string): string {
	return KNOWN_EVENTS.find((e) => e.value === on)?.label ?? on;
}

/** The distinct modes of a workflow's agent nodes, in node order. */
export function workflowModes(wf: WorkflowDoc): string[] {
	const modes = wf.nodes
		.filter((n: NodeDoc) => n.type === 'agent' && n.mode)
		.map((n: NodeDoc) => n.mode);
	return [...new Set(modes)];
}

import type { Tone } from '#lib/components/ui/status.ts';

/** Workflow runs as enzarb-tracks reports them (ADR-0029 section 7; `GET /v1/runs`). */

export type RunStatus = 'pending' | 'running' | 'waiting' | 'done' | 'failed' | 'cancelled';

export interface RunSummary {
	id: string;
	workflow: string;
	project: string;
	repo: string;
	/** `{kind, number, author, url}` of the issue or pull request, when the event had one. */
	subject: { kind: string; number: number; url?: string } | null;
	eventId: number;
	sha: string;
	status: RunStatus;
	test: boolean;
	currentNode: string | null;
	/** Why the run ended failed or cancelled (`no_credential`, `timeout`, `stopped`, ...). */
	reason: string | null;
	stopRequested: boolean;
	createdAt: string;
	updatedAt: string;
	finishedAt: string | null;
	/** While status is `waiting`: when the wait times out (null: only the run's deadline). */
	waitUntil?: string | null;
}

export interface NodeUsage {
	input_tokens?: number;
	output_tokens?: number;
	cache_read_input_tokens?: number;
	cache_creation_input_tokens?: number;
	models?: string[];
}

export interface NodeGit {
	repo: string;
	branch: string;
	head_sha: string;
	pushed: boolean;
}

/** The node as tracks rendered it before sending it to the sandbox. */
export interface NodeRendered {
	type: 'agent' | 'command' | 'action' | 'wait' | 'end';
	mode?: string;
	instruction?: string;
	push?: boolean;
	task?: string;
	args?: string[];
	script?: string;
	/** `action` nodes: comment, open_pull_request, add_labels, remove_labels. */
	action?: string;
	title?: string;
	body?: string;
	labels?: string[];
	draft?: boolean;
	/** `wait` nodes. */
	on?: string[];
	condition?: string;
	timeout_secs?: number;
	[k: string]: unknown;
}

/** What a `wait` node waits for, and how it ended (`node_runs.wait`). */
export interface NodeWait {
	on: string[];
	if: string | null;
	timeout_secs: number | null;
	/** When the wait times out; null without a timeout. */
	until: string | null;
	resumed_by_event?: number | null;
	timed_out?: boolean;
}

export interface NodeRun {
	node: string;
	visit: number;
	status: string;
	rendered: NodeRendered | null;
	mode: string | null;
	outputs: Record<string, unknown> | null;
	usage: NodeUsage | null;
	git: NodeGit[] | null;
	error: string | null;
	/** The GitHub call an `action` node made (or, in a test run, would have made). */
	intent: Record<string, unknown> | null;
	wait: NodeWait | null;
	startedAt: string | null;
	finishedAt: string | null;
}

export interface RunDetail extends RunSummary {
	nodeRuns: NodeRun[];
}

export const RUN_ENDED: RunStatus[] = ['done', 'failed', 'cancelled'];

export const isEnded = (r: Pick<RunSummary, 'status'>) => RUN_ENDED.includes(r.status);

export function runTone(status: string): { tone: Tone; hollow?: boolean } {
	switch (status) {
		case 'done':
			return { tone: 'success' };
		case 'failed':
			return { tone: 'danger' };
		case 'cancelled':
			return { tone: 'neutral' };
		case 'running':
			return { tone: 'info' };
		default:
			return { tone: 'neutral', hollow: true };
	}
}

/** `branch` of `repo` on GitHub, or null for branches that were never pushed. */
export function branchUrl(g: NodeGit): string | null {
	return g.pushed && g.branch
		? `https://github.com/${g.repo}/tree/${g.branch.split('/').map(encodeURIComponent).join('/')}`
		: null;
}

/** What the command or agent line of a node shows: the rendered instruction or command. */
export function nodeText(r: NodeRendered | null): string {
	if (!r) return '';
	if (r.type === 'agent') return r.instruction ?? '';
	if (r.type === 'command') {
		if (r.script) return r.script;
		return ['mise run', r.task, ...(r.args?.length ? ['--', ...r.args] : [])]
			.filter(Boolean)
			.join(' ');
	}
	return '';
}

export function usageText(u: NodeUsage | null): string {
	if (!u) return '';
	const parts: string[] = [];
	if (u.input_tokens !== undefined) parts.push(`${u.input_tokens} in`);
	if (u.output_tokens !== undefined) parts.push(`${u.output_tokens} out`);
	return parts.join(', ');
}

const ACTION_LABELS: Record<string, string> = {
	comment: 'Commented',
	open_pull_request: 'Opened pull request',
	add_labels: 'Added labels',
	remove_labels: 'Removed labels'
};

/** What an `action` node did, as a sentence ("Opened pull request #42"); empty for other nodes. */
export function actionText(n: Pick<NodeRun, 'rendered' | 'outputs' | 'status'>): string {
	const r = n.rendered;
	if (r?.type !== 'action' || !r.action) return '';
	const test = n.outputs?.test === true;
	const num = n.outputs?.number;
	const what = ACTION_LABELS[r.action] ?? r.action;
	const labels = r.labels?.length ? `: ${r.labels.join(', ')}` : '';
	if (n.status !== 'done') return `${r.action.replace(/_/g, ' ')} did not complete`;
	const ref = r.action === 'open_pull_request' && num !== undefined ? ` #${num}` : '';
	return `${test ? 'Would have: ' : ''}${what}${ref}${labels}`;
}

/** A GitHub link the action produced (comment, pull request); null for labels and test runs. */
export function actionUrl(n: Pick<NodeRun, 'rendered' | 'outputs'>): string | null {
	const u = n.outputs?.url;
	if (n.rendered?.type !== 'action' || typeof u !== 'string' || n.outputs?.test === true)
		return null;
	return /^https:\/\/github\.com\//.test(u) ? u : null;
}

/** "waiting for issue_comment.created if <condition>, until <time>" for a parked or finished wait. */
export function waitText(n: Pick<NodeRun, 'rendered' | 'wait'>): string {
	const w = n.wait;
	const on = w?.on ?? n.rendered?.on ?? [];
	if (on.length === 0) return '';
	const cond = w?.if ?? n.rendered?.condition;
	return `${on.join(', ')}${cond ? ` if ${cond}` : ''}`;
}

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
	[k: string]: unknown;
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

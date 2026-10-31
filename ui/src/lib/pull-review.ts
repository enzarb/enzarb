import type { PullFile } from './patch.ts';

/** What the pull request view shows and what merging needs to know (see `tasks-review.ts`). */

export interface CheckRun {
	name: string;
	status: string;
	conclusion: string | null;
	url: string;
}

export interface PullState {
	number: number;
	title: string;
	url: string;
	draft: boolean;
	headSha: string;
	/** null: GitHub has not worked it out yet. */
	mergeable: boolean | null;
	mergeableState: string;
}

export interface CheckSummary {
	passed: number;
	failed: string[];
	pending: string[];
}

const GOOD = new Set(['success', 'neutral', 'skipped']);

export function summarizeChecks(runs: CheckRun[]): CheckSummary {
	const s: CheckSummary = { passed: 0, failed: [], pending: [] };
	for (const r of runs) {
		if (r.status !== 'completed') s.pending.push(r.name);
		else if (r.conclusion && GOOD.has(r.conclusion)) s.passed++;
		else s.failed.push(r.name);
	}
	return s;
}

/** "3 passed, 1 failed (lint), 2 pending" */
export function checksText(s: CheckSummary): string {
	if (s.passed + s.failed.length + s.pending.length === 0) return 'No checks ran';
	const parts = [`${s.passed} passed`];
	if (s.failed.length) parts.push(`${s.failed.length} failed (${s.failed.join(', ')})`);
	if (s.pending.length) parts.push(`${s.pending.length} pending`);
	return parts.join(', ');
}

/** Why the pull request must not be merged right now; null when it may. */
export function mergeBlock(pull: PullState, checks: CheckSummary): string | null {
	if (pull.draft) return 'The pull request is still a draft.';
	if (pull.mergeable === false || pull.mergeableState === 'dirty')
		return 'The pull request has merge conflicts.';
	if (checks.failed.length) return `Checks are failing: ${checks.failed.join(', ')}.`;
	if (checks.pending.length) return `Checks are still running: ${checks.pending.join(', ')}.`;
	if (pull.mergeable === null)
		return 'GitHub is still working out whether it can be merged. Try again in a moment.';
	return null;
}

/** The approval review's body: it is the App's review, so it says who approved. */
export const approvalBody = (login: string) => `Approved by @${login} in enzarb`;

/** The squash commit title. */
export const mergeTitle = (title: string, number: number) => `${title} (#${number})`;

export interface ReviewNotes {
	html: string;
	/** The run that wrote them, when they came from a run. */
	run: { id: string; workflow: string } | null;
	source: 'run' | 'comment';
}

export interface PullReview {
	pull: PullState | null;
	files: PullFile[];
	filesTruncated: boolean;
	checks: CheckRun[];
	notes: ReviewNotes | null;
	/** Parts GitHub could not give, for a notice. */
	problems: string[];
}

export type StepResult = 'done' | 'failed' | 'skipped';
export interface MergeStep {
	id: 'access' | 'approve' | 'status' | 'merge';
	label: string;
	result: StepResult;
	message?: string;
}
export interface MergeOutcome {
	ok: boolean;
	message: string;
	steps: MergeStep[];
}

import * as v from 'valibot';

/** Actions accepted by `workflows_api.submit_action` (contracts §6). */
export const ACTIONS = ['approve', 'cancel', 'retry', 'merge'] as const;
export type WorkAction = (typeof ACTIONS)[number];

export const WorkActionSchema = v.picklist(ACTIONS);

export const SubmitActionSchema = v.object({
	id: v.pipe(v.number(), v.integer(), v.minValue(1)),
	action: WorkActionSchema
});

export interface WorkItemSummary {
	id: number;
	key: string;
	project: string;
	workflow: string;
	state: string;
	title: string;
	url: string | null;
	updatedAt: string;
}

export interface TransitionView {
	id: number;
	fromState: string | null;
	toState: string;
	eventType: string;
	at: string;
}

export interface JobView {
	id: number;
	kind: string;
	status: string;
	attempt: number;
	updatedAt: string;
	/** Sanitized HTML rendered from the job output, or null when there is none. */
	outputHtml: string | null;
}

export interface WorkItemDetail extends WorkItemSummary {
	actions: WorkAction[];
	transitions: TransitionView[];
	jobs: JobView[];
	/** Merge readiness for change requests, null for every other workflow. */
	readiness: Readiness | null;
}

export const CHECK_STATUSES = ['ok', 'pending', 'failed', 'unknown'] as const;
export type CheckStatus = (typeof CHECK_STATUSES)[number];

export interface CheckResult {
	status: CheckStatus;
	detail: string | null;
}

/** Engine-computed merge readiness (`state_data.checklist`) and why the last merge did not happen. */
export interface Readiness {
	ci: CheckResult;
	review: CheckResult;
	mergeable: CheckResult;
	mergeError: string | null;
}

const UNKNOWN: CheckResult = { status: 'unknown', detail: null };

const CheckSchema = v.object({ status: v.string(), detail: v.optional(v.unknown()) });

function parseCheck(raw: unknown): CheckResult {
	const r = v.safeParse(CheckSchema, raw);
	if (!r.success) return UNKNOWN;
	const status = r.output.status.toLowerCase();
	if (!(CHECK_STATUSES as readonly string[]).includes(status)) return UNKNOWN;
	const detail = typeof r.output.detail === 'string' ? r.output.detail.trim() : '';
	return { status: status as CheckStatus, detail: detail || null };
}

const asRecord = (x: unknown): Record<string, unknown> =>
	typeof x === 'object' && x !== null ? (x as Record<string, unknown>) : {};

/**
 * The only code that knows the readiness shape of a change request's `state_data`
 * (`checklist.{ci,review,mergeable}` = `{status, detail}`, `merge_error`). Never throws: anything
 * unexpected reads as `unknown`.
 */
export function parseReadiness(stateData: unknown): Readiness {
	const data = asRecord(stateData);
	const list = asRecord(data.checklist);
	const err = typeof data.merge_error === 'string' ? data.merge_error.trim() : '';
	return {
		ci: parseCheck(list.ci),
		review: parseCheck(list.review),
		mergeable: parseCheck(list.mergeable),
		mergeError: err || null
	};
}

/** Splits items into the "Ready to merge" group and the rest, keeping each group's order. */
export function groupWorkItems<T extends { state: string }>(
	items: readonly T[]
): { ready: T[]; rest: T[] } {
	const ready: T[] = [];
	const rest: T[] = [];
	for (const i of items) (i.state === 'ReadyToMerge' ? ready : rest).push(i);
	return { ready, rest };
}

/** States no action can leave (engine `machine/*`: issue, change_request, mention phases). */
const TERMINAL = new Set(['Done', 'Duplicate', 'Cancelled', 'Merged', 'Closed']);

export const isTerminal = (state: string): boolean => TERMINAL.has(state);

/**
 * Actions valid in a state: approve only while AwaitingApproval, merge only while ReadyToMerge, retry when Failed or Stale, cancel
 * for every non-terminal state.
 */
export function availableActions(state: string): WorkAction[] {
	const out: WorkAction[] = [];
	if (state === 'AwaitingApproval') out.push('approve');
	if (state === 'Failed' || state === 'Stale') out.push('retry');
	if (state === 'ReadyToMerge') out.push('merge');
	if (!isTerminal(state)) out.push('cancel');
	return out;
}

export type StateTone = 'active' | 'waiting' | 'done' | 'failed' | 'ready';

export function stateTone(state: string): StateTone {
	if (state === 'Failed') return 'failed';
	if (state === 'ReadyToMerge') return 'ready';
	if (isTerminal(state)) return state === 'Cancelled' ? 'failed' : 'done';
	if (
		state === 'AwaitingApproval' ||
		state === 'AwaitingAnswers' ||
		state === 'AwaitingChanges' ||
		state === 'Stale'
	)
		return 'waiting';
	return 'active';
}

/**
 * Upper bound on the work items that may hold a running pod: every item in the `active` tone. The
 * workflows contract records "running" on jobs (`workflows.jobs.status`), not on work-item states, and
 * the machine states live in enzarb-workflows, so queued/planning items cannot be told apart here.
 * Callers must treat the result as approximate (zero / non-zero only), never as an exact count.
 */
export const countPossiblyRunning = (items: readonly { state: string }[]): number =>
	items.filter((w) => stateTone(w.state) === 'active').length;

/** `github:org/repo#12` -> issue URL; `github:org/repo!12` is not produced for GitHub. */
export function keyToUrl(key: string): string | null {
	const m = /^github:([\w.-]+\/[\w.-]+)#(\d+)$/.exec(key);
	return m ? `https://github.com/${m[1]}/issues/${m[2]}` : null;
}

/** Prefer `state_data.url` when it is an https URL, else derive it from the key. */
export function itemUrl(key: string, stateData: unknown): string | null {
	const url = (stateData as { url?: unknown } | null)?.url;
	if (typeof url === 'string') {
		try {
			const u = new URL(url);
			if (u.protocol === 'https:') return u.href;
		} catch {
			/* fall through */
		}
	}
	return keyToUrl(key);
}

export function itemTitle(key: string, stateData: unknown): string {
	const title = (stateData as { title?: unknown } | null)?.title;
	return typeof title === 'string' && title.trim() ? title : key;
}

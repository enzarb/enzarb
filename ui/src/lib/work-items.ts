import * as v from 'valibot';

/** Actions accepted by `workflows_api.submit_action` (contracts §6). */
export const ACTIONS = ['approve', 'cancel', 'retry'] as const;
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
}

/** States no action can leave (engine `machine/*`: issue, change_request, mention phases). */
const TERMINAL = new Set(['Done', 'Duplicate', 'Cancelled', 'Merged', 'Closed']);

export const isTerminal = (state: string): boolean => TERMINAL.has(state);

/**
 * Actions valid in a state: approve only while AwaitingApproval, retry only when Failed, cancel
 * for every non-terminal state.
 */
export function availableActions(state: string): WorkAction[] {
	const out: WorkAction[] = [];
	if (state === 'AwaitingApproval') out.push('approve');
	if (state === 'Failed') out.push('retry');
	if (!isTerminal(state)) out.push('cancel');
	return out;
}

export type StateTone = 'active' | 'waiting' | 'done' | 'failed';

export function stateTone(state: string): StateTone {
	if (state === 'Failed') return 'failed';
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

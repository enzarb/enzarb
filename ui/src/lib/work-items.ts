import * as v from 'valibot';
import type { Usage } from './usage.ts';

/** Actions accepted by `workflows_api.submit_action` (contracts §6). */
export const ACTIONS = ['approve', 'cancel', 'retry', 'merge', 'comment'] as const;
export type WorkAction = (typeof ACTIONS)[number];

export const WorkActionSchema = v.picklist(ACTIONS);

/** Longest comment `submit_action` accepts (contracts §6). */
export const COMMENT_MAX_LENGTH = 20_000;

/** The optional payload of an action: `comment` needs `body`, `merge` may set `deleteBranch`. */
export const ActionDataSchema = v.object({
	body: v.optional(v.pipe(v.string(), v.trim(), v.minLength(1), v.maxLength(COMMENT_MAX_LENGTH))),
	deleteBranch: v.optional(v.boolean())
});
export type ActionData = v.InferOutput<typeof ActionDataSchema>;

export const SubmitActionSchema = v.pipe(
	v.object({
		id: v.pipe(v.number(), v.integer(), v.minValue(1)),
		action: WorkActionSchema,
		data: v.optional(ActionDataSchema)
	}),
	v.check((i) => i.action !== 'comment' || !!i.data?.body, 'A comment needs text')
);

export interface WorkItemSummary {
	id: number;
	key: string;
	project: string;
	workflow: string;
	state: string;
	title: string;
	url: string | null;
	updatedAt: string;
	/** Set when the item is waiting on the signed-in user (see `classifyNeedsYou`). */
	needs?: NeedsYou | null;
	/** Why the item is waiting, when workflows derived it (change requests). */
	wait?: Wait | null;
	/** Token usage and cost summed over the item's jobs; null when no job reported any. */
	usage?: Usage | null;
	/** True when the item's project has exactly one repository; unset when unknown. */
	singleRepo?: boolean;
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
	/** Token usage and cost over the job's runs; null when the agent reported none. */
	usage?: Usage | null;
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
export const TERMINAL_STATES = ['Done', 'Duplicate', 'Cancelled', 'Merged', 'Closed'] as const;

const TERMINAL = new Set<string>(TERMINAL_STATES);

export const isTerminal = (state: string): boolean => TERMINAL.has(state);

const isCiFailedWait = (wait: Wait | null): boolean =>
	wait?.waitingOn === 'human' && wait.reason === 'ci_failed';

// `classifyNeedsYou` and `availableActions` are mirrored in SQL by `needsYouWhere` in
// `server/work-service.ts`; change them together (a parity spec guards the two).

/**
 * Actions valid in a state: approve only while AwaitingApproval, merge only while ReadyToMerge, retry when Failed or Stale, cancel
 * for every non-terminal state. A change request waiting on `ci_failed` (no fix round is running)
 * can also retry, which starts another CI-fix round; that needs the `state_data`.
 */
export function availableActions(state: string, stateData?: unknown): WorkAction[] {
	const out: WorkAction[] = [];
	if (state === 'AwaitingApproval') out.push('approve');
	if (state === 'Failed' || state === 'Stale') out.push('retry');
	else if (state === 'AwaitingChanges' && isCiFailedWait(parseWait(stateData))) out.push('retry');
	if (state === 'ReadyToMerge') out.push('merge');
	if (!isTerminal(state)) out.push('cancel');
	return out;
}

/**
 * Whether feedback can be sent to the item: only an issue's plan awaiting approval or questions
 * awaiting answers take a comment (the engine rejects it elsewhere). Kept apart from
 * `availableActions`, which lists the buttons of the task page.
 */
export const canComment = (workflow: string, state: string): boolean =>
	workflow === 'issue' && (state === 'AwaitingApproval' || state === 'AwaitingAnswers');

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

/**
 * The work item id in a `/work/[id]` route param, or null when it is not a canonical positive safe
 * integer (missing, `abc`, `0`, `-3`, `1.5`, `007`, ...). Callers must not make a remote call for null.
 */
export function parseWorkId(param: string | undefined): number | null {
	if (param === undefined || !/^[1-9]\d*$/.test(param)) return null;
	const n = Number(param);
	return Number.isSafeInteger(n) ? n : null;
}

/** Whether a live change (`null` = refresh everything) concerns the item shown on the detail page. */
export const shouldRefresh = (changed: number | null, id: number): boolean =>
	changed === null || changed === id;

/** List order is `updated_at DESC, id DESC`. */
const byRecency = (a: WorkItemSummary, b: WorkItemSummary): number =>
	Date.parse(b.updatedAt) - Date.parse(a.updatedAt) || b.id - a.id;

/**
 * Apply one changed item to the list: replace its row (or insert it) at its sorted position.
 * An item outside the project filter is left out, and dropped if it was listed before.
 */
export function applyItemChange(
	list: WorkItemSummary[],
	item: WorkItemSummary,
	project?: string
): WorkItemSummary[] {
	const rest = list.filter((w) => w.id !== item.id);
	if (project && item.project !== project) return rest;
	return [...rest, item].sort(byRecency);
}

export const WAITING_ON = ['human', 'enzarb', 'ci', 'github'] as const;
export type WaitingOn = (typeof WAITING_ON)[number];

/** Why a change request is waiting (`state_data.wait`, derived by workflows). */
export interface Wait {
	waitingOn: WaitingOn;
	/** Stable code, e.g. `awaiting_review`, `ci_pending` (contracts §6). */
	reason: string;
	text: string;
	/** Link target of the reason (the comment a decision is waiting on), https only. */
	url: string | null;
}

const httpsUrl = (x: unknown): string | null => {
	if (typeof x !== 'string') return null;
	try {
		const u = new URL(x);
		return u.protocol === 'https:' ? u.href : null;
	} catch {
		return null;
	}
};

/** The `wait` of a work item's `state_data`; null when absent (older workflows) or malformed. */
export function parseWait(stateData: unknown): Wait | null {
	const w = asRecord(asRecord(stateData).wait);
	const on = w.waiting_on;
	if (typeof on !== 'string' || !(WAITING_ON as readonly string[]).includes(on)) return null;
	return {
		waitingOn: on as WaitingOn,
		reason: typeof w.reason === 'string' ? w.reason : '',
		text: typeof w.text === 'string' ? w.text.trim() : '',
		url: httpsUrl(w.url)
	};
}

export interface WaitLink {
	label: string;
	href: string;
}

/** The pull request URL itself; the issue URL derived from a key is not one. */
const prUrl = (url: string | null): string | null =>
	url && /^https:\/\/[^/]+\/[^/]+\/[^/]+\/pull\/\d+$/.test(url) ? url : null;

/** `owner/repo#n` (or `#n` for a single-repo project) for a GitHub issue or PR URL; null otherwise. */
export function githubRefLabel(url: string | null, singleRepo?: boolean): string | null {
	const m = url && /^https:\/\/github\.com\/([^/]+\/[^/]+)\/(?:issues|pull)\/(\d+)$/.exec(url);
	if (!m) return null;
	return singleRepo ? `#${m[2]}` : `${m[1]}#${m[2]}`;
}

/** Links that help the user act on (or follow) a wait; `url` is the item's URL. */
export function waitLinks(wait: Wait | null, url: string | null): WaitLink[] {
	if (!wait) return [];
	const pr = prUrl(url);
	switch (wait.reason) {
		case 'awaiting_review':
		case 'checks_unverified':
			return pr ? [{ label: 'Review on GitHub', href: `${pr}/files` }] : [];
		case 'ci_pending':
		case 'ci_stale':
			return pr ? [{ label: 'View checks', href: `${pr}/checks` }] : [];
		case 'ci_failed':
			if (wait.url) return [{ label: 'View failing check', href: wait.url }];
			return pr ? [{ label: 'View failing check', href: `${pr}/checks` }] : [];
		case 'enzarb_failed':
		case 'round_limit':
		case 'conflict_parked':
			return pr ? [{ label: 'Open PR', href: pr }] : [];
		case 'decision_needed':
			return wait.url ? [{ label: 'View comment', href: wait.url }] : [];
		default:
			return [];
	}
}

export const NEEDS_YOU_CATEGORIES = [
	'approval',
	'merge',
	'review',
	'decision',
	'blocked',
	'failed',
	'question'
] as const;
export type NeedsYouCategory = (typeof NEEDS_YOU_CATEGORIES)[number];

/** Why an item is waiting on the user; `reason` is the failure / blocker text when there is one. */
export interface NeedsYou {
	category: NeedsYouCategory;
	reason: string | null;
	/** Specific reason text from `state_data.wait`; absent on the state-based fallback. */
	text?: string;
}

const failedChecks = (stateData: unknown) => {
	const r = parseReadiness(stateData);
	return (
		[
			['CI', r.ci],
			['Review', r.review],
			['Merge', r.mergeable]
		] as const
	).filter(([, c]) => c.status === 'failed');
};

/** What blocks a change request: the first failed check's detail, else the last merge error. */
export function blockedReason(stateData: unknown): string | null {
	const [first] = failedChecks(stateData);
	if (first) return first[1].detail ? `${first[0]}: ${first[1].detail}` : `${first[0]} failed`;
	return parseReadiness(stateData).mergeError;
}

const failureText = (stateData: unknown): string | null => {
	const data = asRecord(stateData);
	for (const k of ['error', 'failure_reason', 'reason']) {
		const x = data[k];
		if (typeof x === 'string' && x.trim()) return x.trim();
	}
	return null;
};

/** Hours a blocked change request may sit untouched before it needs its owner (`NEEDS_YOU_STALE_HOURS`). */
export const NEEDS_YOU_STALE_HOURS_DEFAULT = 24;

/** Phases where enzarb itself is acting on a change request, so failed checks are not the owner's to fix. */
export const ENZARB_WORKING_PHASES = [
	'Reviewing',
	'AddressingComments',
	'ResolvingConflicts',
	'Merging'
] as const;

const WORKING = new Set<string>(ENZARB_WORKING_PHASES);

export interface NeedsYouOptions {
	/** Epoch ms treated as the present. */
	now: number;
	/** A blocked change request counts only after this long without activity. */
	staleAfterMs: number;
}

export const hoursToMs = (hours: number): number => hours * 3_600_000;

/**
 * Whether (and why) an item waits on its owner. Never throws; unknown states and malformed
 * `state_data` classify as `null`. A change request that is blocked (changes requested, red checks)
 * waits on enzarb, not the owner, until it has had no activity (`updatedAt`) for `staleAfterMs`;
 * `ConflictParked` always needs a decision.
 */
export function classifyNeedsYou(
	item: {
		workflow: string;
		state: string;
		stateData: unknown;
		updatedAt?: string | number | Date;
	},
	opts: Partial<NeedsYouOptions> = {}
): NeedsYou | null {
	const { workflow, state, stateData } = item;
	if (isTerminal(state)) return null;
	const actions = availableActions(state, stateData);
	if (actions.includes('retry')) {
		const wait = workflow === 'change_request' ? parseWait(stateData) : null;
		if (isCiFailedWait(wait))
			return {
				category: 'failed',
				reason: wait?.text || 'CI failed',
				text: wait?.text || undefined
			};
		return { category: 'failed', reason: failureText(stateData) };
	}
	if (workflow === 'issue' && actions.includes('approve'))
		return { category: 'approval', reason: null };
	if (workflow === 'issue' && state === 'AwaitingAnswers')
		return { category: 'question', reason: null };
	if (workflow !== 'change_request') return null;
	const wait = parseWait(stateData);
	if (wait) {
		// workflows says exactly who is waited on; only a human counts as "needs you"
		if (wait.waitingOn !== 'human') return null;
		const text = wait.text || undefined;
		if (wait.reason === 'ready_to_merge') return { category: 'merge', reason: null, text };
		if (wait.reason === 'awaiting_review' || wait.reason === 'checks_unverified')
			return { category: 'review', reason: null, text };
		if (wait.reason === 'decision_needed') return { category: 'decision', reason: null, text };
		return { category: 'blocked', reason: blockedReason(stateData), text };
	}
	if (actions.includes('merge')) return { category: 'merge', reason: null };
	if (state === 'ConflictParked')
		return { category: 'blocked', reason: 'Merge conflict needs a decision' };
	if (WORKING.has(state)) return null;
	if (state !== 'AwaitingChanges' && failedChecks(stateData).length === 0) return null;
	const now = opts.now ?? Date.now();
	const staleAfterMs = opts.staleAfterMs ?? hoursToMs(NEEDS_YOU_STALE_HOURS_DEFAULT);
	const since = item.updatedAt === undefined ? NaN : new Date(item.updatedAt).getTime();
	if (!Number.isFinite(since) || now - since < staleAfterMs) return null;
	return { category: 'blocked', reason: blockedReason(stateData) ?? 'Changes requested' };
}

/** The action a queue row offers inline; review, decision, blocked and question rows only link out. */
export function primaryAction(category: NeedsYouCategory): WorkAction | null {
	if (category === 'approval') return 'approve';
	if (category === 'merge') return 'merge';
	if (category === 'failed') return 'retry';
	return null;
}

/** Oldest-waiting first. */
const byAge = (a: WorkItemSummary, b: WorkItemSummary): number =>
	Date.parse(a.updatedAt) - Date.parse(b.updatedAt) || a.id - b.id;

/** Apply one changed item to the needs-you list: insert, replace, or drop it once it no longer needs the user. */
export function applyNeedsYouChange(
	list: WorkItemSummary[],
	item: WorkItemSummary
): WorkItemSummary[] {
	const rest = list.filter((w) => w.id !== item.id);
	return item.needs ? [...rest, item].sort(byAge) : rest;
}

/**
 * The columns of the Tasks board, in process order (left to right): plan approval, the work
 * itself, what got stuck, review and merge, done. The array order is the render order, so
 * keyboard and screen-reader order match the picture.
 */
export const BOARD_COLUMNS = [
	{ id: 'plan', title: 'Plan approval', needsYou: true },
	{ id: 'progress', title: 'In progress', needsYou: false },
	{ id: 'blocked', title: 'Blocked / failed', needsYou: true },
	{ id: 'review', title: 'Review & merge', needsYou: true },
	{ id: 'done', title: 'Done', needsYou: false }
] as const;
/** How many finished tasks the Done column lists before "Show all". */
export const DONE_LIMIT = 20;

export type BoardColumnId = (typeof BOARD_COLUMNS)[number]['id'];

const NEEDS_COLUMN: Record<NeedsYouCategory, BoardColumnId> = {
	approval: 'plan',
	question: 'plan',
	blocked: 'blocked',
	failed: 'blocked',
	merge: 'review',
	review: 'review',
	decision: 'review'
};

/**
 * The column of an item. Items waiting on the user sit by their `needs` category (the result of
 * `classifyNeedsYou`; the board has no classifier of its own); everything else is in progress,
 * or done once the state is terminal. Unknown states count as in progress.
 */
export function boardColumn(item: { state: string; needs?: NeedsYou | null }): BoardColumnId {
	if (item.needs) return NEEDS_COLUMN[item.needs.category];
	return isTerminal(item.state) ? 'done' : 'progress';
}

/** Items per column, each keeping the input order; every column is present. */
export function groupByColumn<T extends { state: string; needs?: NeedsYou | null }>(
	items: readonly T[]
): Record<BoardColumnId, T[]> {
	const out: Record<BoardColumnId, T[]> = {
		plan: [],
		progress: [],
		blocked: [],
		review: [],
		done: []
	};
	for (const i of items) out[boardColumn(i)].push(i);
	return out;
}

/**
 * What the board shows: the needs-you queue (filtered to `project`) plus the listed items that
 * are not waiting on the user. An id is listed once, the queue's copy wins, so a card cannot
 * appear twice and a state change moves it between columns. Newest first.
 */
export function boardItems(
	queue: readonly WorkItemSummary[],
	list: readonly WorkItemSummary[],
	project?: string
): WorkItemSummary[] {
	const waiting = queue.filter((w) => !project || w.project === project);
	const ids = new Set(waiting.map((w) => w.id));
	return [...waiting, ...list.filter((w) => !w.needs && !ids.has(w.id))].sort(byRecency);
}

/**
 * Tasks: everything that waits on a human (ADR-0029). Four kinds, all derived from tracks' runs
 * and the GitHub App's live view of the Project's repositories; nothing is stored.
 */

/** The workflow the Implement button starts (the template's id, the key in `.ez/workflows.toml`). */
export const IMPLEMENT_WORKFLOW = 'implement-labeled-issues';
/** The label triage puts on an issue it has understood. */
export const TRIAGE_LABEL = 'triage';
/** Branch prefix of the pull requests enzarb's runs open (`ez/<workflow>/<run>`). */
export const RUN_BRANCH_PREFIX = 'ez/';

interface Base {
	/**
	 * Stable key within one list, and the `?task=` value in the URL: `run:<id>` (waiting or failed
	 * run), `pr:<owner/repo>#<n>`, `issue:<owner/repo>#<n>`.
	 */
	key: string;
	project: string;
	/** `owner/repo` */
	repo: string;
	/** Issue or pull request number the task is about, when it has one. */
	number: number | null;
	title: string;
	/** The issue or pull request on GitHub. */
	githubUrl: string | null;
	/** The enzarb run page (`/projects/<name>/runs/<id>`), when a run is involved. */
	runUrl: string | null;
	/** Newest first within a kind. */
	at: string;
}

/** A run parked at a `wait` node. */
export interface WaitTask extends Base {
	kind: 'wait';
	runId: string;
	workflow: string;
	/** "issue_comment.created if <condition>" */
	waitingFor: string;
	/** When the wait times out; null without a timeout. */
	until: string | null;
}

/** A run that ended failed. */
export interface FailedTask extends Base {
	kind: 'failed';
	runId: string;
	workflow: string;
	reason: string | null;
}

/** An open pull request from an `ez/*` branch. */
export interface ReviewTask extends Base {
	kind: 'review';
	branch: string;
	draft: boolean;
}

/** An issue labelled `triage` that no implement run has started on. */
export interface TriagedTask extends Base {
	kind: 'triaged';
	/** The issue text from GitHub (cached server-side). */
	body?: string;
}

export type Task = WaitTask | FailedTask | ReviewTask | TriagedTask;
export type TaskKind = Task['kind'];

export interface TaskList {
	tasks: Task[];
	/** Parts that could not be read (GitHub rate limit, tracks down), for a notice. */
	notes: string[];
}

export const KIND_LABELS: Record<TaskKind, string> = {
	wait: 'Waiting for you',
	failed: 'Failed runs',
	review: 'Pull requests to review',
	triaged: 'Triaged issues'
};

export const KIND_ORDER: TaskKind[] = ['wait', 'failed', 'review', 'triaged'];

export function groupTasks(tasks: Task[]): { kind: TaskKind; label: string; tasks: Task[] }[] {
	return KIND_ORDER.map((kind) => ({
		kind,
		label: KIND_LABELS[kind],
		tasks: tasks.filter((t) => t.kind === kind)
	})).filter((g) => g.tasks.length > 0);
}

export type DefaultActionId = 'implement' | 'retry' | 'stop' | 'diff';

/** The primary button of a task's detail. */
export function defaultAction(t: Task): { id: DefaultActionId; label: string } {
	switch (t.kind) {
		case 'triaged':
			return { id: 'implement', label: 'Implement' };
		case 'failed':
			return { id: 'retry', label: 'Retry' };
		case 'wait':
			return { id: 'stop', label: 'Stop for now' };
		case 'review':
			return { id: 'diff', label: 'Open diff' };
	}
}

/** Short status for a sidebar entry. */
export function shortStatus(t: Task): string {
	switch (t.kind) {
		case 'wait':
			return 'waiting';
		case 'failed':
			return t.reason ? `failed: ${t.reason}` : 'failed';
		case 'review':
			return t.draft ? 'draft PR' : 'open PR';
		case 'triaged':
			return 'triaged';
	}
}

/** What a sidebar entry is, in a word. */
export const KIND_NAMES: Record<TaskKind, string> = {
	wait: 'Run',
	failed: 'Run',
	review: 'Pull request',
	triaged: 'Issue'
};

/** The run behind the task with this key (`run:<id>`), for loading its nodes. */
export function runOfTask(
	list: TaskList | undefined,
	key: string | null
): { project: string; id: string } | null {
	const t = list?.tasks.find((x) => x.key === key);
	return t && (t.kind === 'wait' || t.kind === 'failed')
		? { project: t.project, id: t.runId }
		: null;
}

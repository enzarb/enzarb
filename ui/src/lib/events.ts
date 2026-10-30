/** Webhook deliveries logged by the connectors receiver (`connectors.event_v`). */

export interface EventRow {
	id: number;
	/** GitHub event name: `issues`, `push`, `pull_request`, ... */
	event: string;
	action: string | null;
	/** Lowercase `owner/repo`. */
	repo: string;
	sender: string | null;
	/** One line describing the subject (title, branch, commit message), when the payload has one. */
	summary: string | null;
	receivedAt: string;
}

/** `https://github.com/Owner/Repo(.git)` to the lowercase `owner/repo` key the receiver stores. */
export function repoKeyFromUrl(url: string): string | null {
	try {
		const u = new URL(url);
		const m = /^\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/.exec(u.pathname);
		return m ? `${m[1]}/${m[2]}`.toLowerCase() : null;
	} catch {
		return null;
	}
}

const MAX_SUMMARY = 160;

function text(v: unknown): string | null {
	return typeof v === 'string' && v.trim() ? v.trim() : null;
}

function obj(v: unknown): Record<string, unknown> | null {
	return v !== null && typeof v === 'object' ? (v as Record<string, unknown>) : null;
}

/** First line of the subject the payload is about; `null` when it has no obvious one. */
export function summarizeEvent(payload: unknown): string | null {
	const p = obj(payload);
	if (!p) return null;
	const candidates = [
		obj(p.issue)?.title,
		obj(p.pull_request)?.title,
		obj(p.release)?.name ?? obj(p.release)?.tag_name,
		obj(p.check_run)?.name,
		obj(p.check_suite)?.head_branch,
		obj(p.workflow_run)?.name,
		obj(p.head_commit)?.message,
		p.ref,
		p.description
	];
	for (const c of candidates) {
		const t = text(c);
		if (t) {
			const line = t.split('\n', 1)[0]!;
			return line.length > MAX_SUMMARY ? `${line.slice(0, MAX_SUMMARY - 1)}…` : line;
		}
	}
	return null;
}

/** One workflow's trigger evaluation (`on` / `if`), as tracks recorded it. */
export interface DecisionReason {
	workflow: string;
	matched: boolean;
	on: string | null;
	reason: string;
}

/** What tracks decided about one event (ADR-0029 section 5; `GET /v1/decisions`). */
export interface EventDecision {
	eventId: number;
	/** `started` (a run was created), `no_match`, `skipped` (the event names no repository), `error`, `resumed` (a run waiting for it went on). */
	outcome: 'started' | 'no_match' | 'skipped' | 'error' | 'resumed';
	project: string | null;
	/** Default-branch commit the workflow files were read at. */
	sha: string | null;
	/** Workflows that started a run. */
	started: string[];
	reasons: DecisionReason[];
	errors: string[];
	/** Runs this event resumed out of a `wait` node. */
	resumed: { run: string; workflow: string; node: string }[];
	decidedAt: string;
}

/** Whether the event started or resumed a run (an undecided event did neither). */
export function isActive(d?: Pick<EventDecision, 'outcome'>): boolean {
	return d?.outcome === 'started' || d?.outcome === 'resumed';
}

/** "Resumed run 1111aaaa (fix-issue, wait_reply)", one per run the event resumed. */
export function resumedText(d: Pick<EventDecision, 'resumed'>): string {
	return d.resumed
		.map((r) => `Resumed run ${r.run.slice(0, 8)} (${r.workflow}, ${r.node})`)
		.join('; ');
}

/** The headline for a decision: "Started fix-issue", "No rule matched", or the failure. */
export function decisionHeadline(d: EventDecision): {
	text: string;
	tone: 'ok' | 'muted' | 'error';
} {
	switch (d.outcome) {
		case 'started':
			return {
				text:
					`Started ${d.started.join(', ')}` + (d.resumed.length > 0 ? `; ${resumedText(d)}` : ''),
				tone: 'ok'
			};
		case 'resumed':
			return { text: resumedText(d), tone: 'ok' };
		case 'error':
			return { text: 'Could not route this event', tone: 'error' };
		case 'skipped':
			return { text: 'Not routed (no repository)', tone: 'muted' };
		default:
			return { text: 'No rule matched', tone: 'muted' };
	}
}

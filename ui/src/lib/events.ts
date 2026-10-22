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

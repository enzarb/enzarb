import type { RepoPermissions } from './github-app.ts';
import { githubFetch } from './github-app.ts';
import type { CheckRun, CommitStatusState, PullState } from '#lib/pull-review.ts';
import type { PullFile } from '#lib/patch.ts';

/**
 * The GitHub reads behind the Tasks page: open pull requests from `ez/*` branches and open issues
 * with a label. Read through the enzarb App's installation tokens, cached per repository for
 * `TTL_MS` (concurrent callers share one request), and rate-limit aware like tracks: after a 403
 * or 429 that is a rate limit, GitHub is not asked again until `retry-after` /
 * `x-ratelimit-reset` has passed; the last good answer (or nothing, with a note) is used meanwhile.
 */

export const TTL_MS = 60_000;
const DEFAULT_LIMIT_WAIT_MS = 60_000;
const TIMEOUT_MS = 10_000;
const MAX_PAGES = 3;
const MAX_BODY = 4000;

export interface OpenPull {
	number: number;
	title: string;
	url: string;
	branch: string;
	draft: boolean;
	updatedAt: string;
}

export interface OpenIssue {
	number: number;
	title: string;
	/** The issue text, cut to `MAX_BODY` characters. */
	body: string;
	/** True when `body` was cut. */
	bodyTruncated: boolean;
	url: string;
	updatedAt: string;
}

export interface GithubReads {
	pulls(repo: string): Promise<{ items: OpenPull[]; note?: string }>;
	issues(repo: string, label: string): Promise<{ items: OpenIssue[]; note?: string }>;
	/** Numbers of every open issue and pull request; `open` is null when GitHub could not tell. */
	openSubjects(repo: string): Promise<{ open: Set<number> | null; note?: string }>;
	/** One pull request (`fresh`: skip the cache, for merge decisions). Null when unreadable. */
	pull(
		repo: string,
		n: number,
		opts?: { fresh?: boolean }
	): Promise<{ pull: PullState | null; note?: string }>;
	/** Changed files with their patches (up to 300; `truncated` when there are more). */
	pullFiles(
		repo: string,
		n: number
	): Promise<{ items: PullFile[]; truncated?: boolean; note?: string }>;
	/** Check runs of a commit. */
	checkRuns(
		repo: string,
		sha: string,
		opts?: { fresh?: boolean }
	): Promise<{ items: CheckRun[]; truncated?: boolean; note?: string }>;
	/** Combined legacy commit status: `none` when the commit has no statuses. Needs `statuses: read`. */
	commitStatus(
		repo: string,
		sha: string,
		opts?: { fresh?: boolean }
	): Promise<{ state: CommitStatusState | null; note?: string }>;
	/** Bodies of the comments on an issue or pull request that carry the `ez-tracks` marker, oldest first. */
	markedComments(repo: string, n: number): Promise<{ items: string[]; note?: string }>;
}

export interface GithubReadsDeps {
	token: (repo: string, perms: RepoPermissions) => Promise<string | null>;
	invalidate: (repo: string, perms: RepoPermissions) => void;
	fetch?: typeof fetch;
	now?: () => number;
}

/** How long to wait after a rate-limited response; null when it is not a rate limit. */
export function rateLimitWait(res: Response, nowMs: number): number | null {
	if (res.status !== 403 && res.status !== 429) return null;
	const retry = Number(res.headers.get('retry-after'));
	if (Number.isFinite(retry) && retry > 0) return retry * 1000;
	if (res.headers.get('x-ratelimit-remaining') === '0') {
		const reset = Number(res.headers.get('x-ratelimit-reset'));
		if (Number.isFinite(reset) && reset > 0) return Math.max(1000, reset * 1000 - nowMs);
		return DEFAULT_LIMIT_WAIT_MS;
	}
	return res.status === 429 ? DEFAULT_LIMIT_WAIT_MS : null;
}

function nextLink(res: Response): string | null {
	for (const part of (res.headers.get('link') ?? '').split(',')) {
		const m = /<([^>]+)>\s*;\s*rel="next"/.exec(part);
		if (m) return m[1];
	}
	return null;
}

export function createGithubReads({
	token,
	invalidate,
	fetch: doFetch = fetch,
	now = Date.now
}: GithubReadsDeps): GithubReads {
	const cache = new Map<string, { at: number; value: unknown[]; truncated: boolean }>();
	const inflight = new Map<
		string,
		Promise<{ items: unknown[]; note?: string; truncated?: boolean }>
	>();
	let blockedUntil = 0;

	async function list<T>(
		repo: string,
		perms: RepoPermissions,
		path: string,
		pick: (raw: Record<string, unknown>) => T | null,
		opts: { field?: string; fresh?: boolean } = {}
	): Promise<{ items: T[]; note?: string; truncated?: boolean }> {
		const key = `${repo.toLowerCase()}\0${path}`;
		const hit = cache.get(key);
		if (!opts.fresh && hit && now() - hit.at < TTL_MS)
			return { items: hit.value as T[], truncated: hit.truncated };
		const stale = (note: string) => ({ items: (hit?.value ?? []) as T[], note });
		if (now() < blockedUntil)
			return stale(`GitHub rate limit: ${repo} is shown from memory until it lifts`);
		// A fresh read never joins a non-fresh request in flight (which may serve cached data).
		const flightKey = opts.fresh ? `${key}\0fresh` : key;
		let p = inflight.get(flightKey) as
			Promise<{ items: T[]; note?: string; truncated?: boolean }> | undefined;
		if (!p) {
			p = (async () => {
				const t = await token(repo, perms);
				if (!t) return stale(`${repo}: the enzarb GitHub App cannot read it`);
				const out: T[] = [];
				let url: string | null = `${path}${path.includes('?') ? '&' : '?'}per_page=100`;
				for (let page = 0; url && page < MAX_PAGES; page++) {
					let res: Response;
					try {
						res = await githubFetch(url, {
							token: t,
							fetch: doFetch,
							redirect: 'error',
							signal: AbortSignal.timeout(TIMEOUT_MS)
						});
					} catch {
						return stale(`${repo}: GitHub could not be reached`);
					}
					const wait = rateLimitWait(res, now());
					if (wait !== null) {
						blockedUntil = now() + wait;
						return stale(`GitHub rate limit: ${repo} is shown from memory until it lifts`);
					}
					if (res.status === 401) invalidate(repo, perms);
					if (!res.ok) return stale(`${repo}: GitHub answered ${res.status}`);
					const body = (await res.json()) as unknown;
					const arr = Array.isArray(body)
						? body
						: opts.field && Array.isArray((body as Record<string, unknown>)?.[opts.field])
							? ((body as Record<string, unknown>)[opts.field] as unknown[])
							: body && typeof body === 'object'
								? [body]
								: [];
					if (arr.length)
						for (const raw of arr) {
							const item = pick(raw as Record<string, unknown>);
							if (item) out.push(item);
						}
					url = nextLink(res);
				}
				// More pages than MAX_PAGES: the list is incomplete.
				const truncated = url !== null;
				cache.set(key, { at: now(), value: out, truncated });
				return { items: out, truncated };
			})().finally(() => inflight.delete(flightKey));
			inflight.set(flightKey, p);
		}
		return p;
	}

	const str = (v: unknown) => (typeof v === 'string' ? v : '');
	const enc = (repo: string) => repo.split('/').map(encodeURIComponent).join('/');

	const num = (v: unknown) => (typeof v === 'number' ? v : 0);

	return {
		pull: async (repo, n, opts) => {
			const r = await list<PullState>(
				repo,
				{ pull_requests: 'read' },
				`/repos/${enc(repo)}/pulls/${n}`,
				(raw) => {
					const sha = str((raw.head as { sha?: unknown } | undefined)?.sha);
					if (typeof raw.number !== 'number' || !sha) return null;
					return {
						number: raw.number,
						title: str(raw.title),
						url: str(raw.html_url),
						draft: raw.draft === true,
						headSha: sha,
						mergeable: typeof raw.mergeable === 'boolean' ? raw.mergeable : null,
						mergeableState: str(raw.mergeable_state)
					};
				},
				{ fresh: opts?.fresh }
			);
			return { pull: r.items[0] ?? null, note: r.note };
		},
		pullFiles: (repo, n) =>
			list<PullFile>(
				repo,
				{ pull_requests: 'read' },
				`/repos/${enc(repo)}/pulls/${n}/files`,
				(r) =>
					str(r.filename)
						? {
								filename: str(r.filename),
								status: str(r.status),
								additions: num(r.additions),
								deletions: num(r.deletions),
								...(typeof r.patch === 'string' ? { patch: r.patch } : {})
							}
						: null
			),
		checkRuns: (repo, sha, opts) =>
			list<CheckRun>(
				repo,
				{ checks: 'read' },
				`/repos/${enc(repo)}/commits/${encodeURIComponent(sha)}/check-runs`,
				(r) => ({
					name: str(r.name),
					status: str(r.status),
					conclusion: typeof r.conclusion === 'string' ? r.conclusion : null,
					url: str(r.html_url)
				}),
				{ field: 'check_runs', fresh: opts?.fresh }
			),
		commitStatus: async (repo, sha, opts) => {
			const r = await list<CommitStatusState>(
				repo,
				{ statuses: 'read' },
				`/repos/${enc(repo)}/commits/${encodeURIComponent(sha)}/status`,
				(raw) => {
					if (num(raw.total_count) === 0) return 'none';
					const s = str(raw.state);
					return s === 'success' || s === 'failure' || s === 'error' || s === 'pending' ? s : null;
				},
				{ fresh: opts?.fresh }
			);
			if (r.note) return { state: null, note: r.note };
			return r.items[0]
				? { state: r.items[0] }
				: { state: null, note: 'The commit status was unreadable' };
		},
		markedComments: (repo, n) =>
			list<string>(
				repo,
				{ pull_requests: 'read' },
				`/repos/${enc(repo)}/issues/${n}/comments`,
				(r) => (str(r.body).includes('<!-- ez-tracks:') ? str(r.body) : null)
			),
		openSubjects: async (repo) => {
			const r = await list<number>(
				repo,
				{ issues: 'read', pull_requests: 'read' },
				`/repos/${enc(repo)}/issues?state=open&sort=updated&direction=desc`,
				(raw) => (typeof raw.number === 'number' ? raw.number : null)
			);
			if (r.note) return { open: null, note: r.note };
			// An incomplete list cannot prove a subject closed: treat the state as unknown.
			return r.truncated ? { open: null } : { open: new Set(r.items) };
		},
		pulls: (repo) =>
			list<OpenPull>(
				repo,
				{ pull_requests: 'read' },
				`/repos/${enc(repo)}/pulls?state=open&sort=updated&direction=desc`,
				(r) => {
					const branch = str((r.head as { ref?: unknown } | undefined)?.ref);
					if (typeof r.number !== 'number' || !branch.startsWith('ez/')) return null;
					return {
						number: r.number,
						title: str(r.title),
						url: str(r.html_url),
						branch,
						draft: r.draft === true,
						updatedAt: str(r.updated_at)
					};
				}
			),
		issues: (repo, label) =>
			list<OpenIssue>(
				repo,
				{ issues: 'read' },
				`/repos/${enc(repo)}/issues?state=open&labels=${encodeURIComponent(label)}&sort=updated&direction=desc`,
				(r) => {
					// The issues endpoint also returns pull requests.
					if (typeof r.number !== 'number' || r.pull_request) return null;
					return {
						number: r.number,
						title: str(r.title),
						body: str(r.body).slice(0, MAX_BODY),
						bodyTruncated: str(r.body).length > MAX_BODY,
						url: str(r.html_url),
						updatedAt: str(r.updated_at)
					};
				}
			)
	};
}

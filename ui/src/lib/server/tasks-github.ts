import type { RepoPermissions } from './github-app.ts';
import { githubFetch } from './github-app.ts';

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
		pick: (raw: Record<string, unknown>) => T | null
	): Promise<{ items: T[]; note?: string; truncated?: boolean }> {
		const key = `${repo.toLowerCase()}\0${path}`;
		const hit = cache.get(key);
		if (hit && now() - hit.at < TTL_MS)
			return { items: hit.value as T[], truncated: hit.truncated };
		const stale = (note: string) => ({ items: (hit?.value ?? []) as T[], note });
		if (now() < blockedUntil)
			return stale(`GitHub rate limit: ${repo} is shown from memory until it lifts`);
		let p = inflight.get(key) as
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
					if (Array.isArray(body))
						for (const raw of body) {
							const item = pick(raw as Record<string, unknown>);
							if (item) out.push(item);
						}
					url = nextLink(res);
				}
				// More pages than MAX_PAGES: the list is incomplete.
				const truncated = url !== null;
				cache.set(key, { at: now(), value: out, truncated });
				return { items: out, truncated };
			})().finally(() => inflight.delete(key));
			inflight.set(key, p);
		}
		return p;
	}

	const str = (v: unknown) => (typeof v === 'string' ? v : '');
	const enc = (repo: string) => repo.split('/').map(encodeURIComponent).join('/');

	return {
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

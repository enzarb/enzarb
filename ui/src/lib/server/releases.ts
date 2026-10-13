export const RELEASE_CACHE_TTL_MS = 10 * 60_000;
export const RELEASE_FETCH_TIMEOUT_MS = 5_000;

export interface Release {
	tag: string;
	name: string;
	publishedAt: string | null;
	url: string;
	body: string;
}

export interface FetchReleasesOptions {
	fetch?: typeof fetch;
	token?: string;
	signal?: AbortSignal;
}

interface GithubRelease {
	tag_name?: unknown;
	name?: unknown;
	published_at?: unknown;
	html_url?: unknown;
	body?: unknown;
	draft?: unknown;
}

/** First page (30) of an enzarb repo's published releases, newest first. Throws on any failure. */
export async function fetchReleases(
	repo: string,
	{ fetch: doFetch = fetch, token, signal }: FetchReleasesOptions = {}
): Promise<Release[]> {
	const headers: Record<string, string> = {
		Accept: 'application/vnd.github+json',
		'User-Agent': 'enzarb-ui'
	};
	if (token) headers.Authorization = `Bearer ${token}`;
	const res = await doFetch(
		`https://api.github.com/repos/enzarb/${encodeURIComponent(repo)}/releases?per_page=30`,
		{ headers, signal: signal ?? AbortSignal.timeout(RELEASE_FETCH_TIMEOUT_MS) }
	);
	if (!res.ok) throw new Error(`GitHub releases for ${repo}: HTTP ${res.status}`);
	const json: unknown = await res.json();
	if (!Array.isArray(json)) throw new Error(`GitHub releases for ${repo}: unexpected response`);
	return (json as GithubRelease[])
		.filter((r) => !r.draft && typeof r.tag_name === 'string')
		.map((r) => ({
			tag: r.tag_name as string,
			name: typeof r.name === 'string' && r.name ? r.name : (r.tag_name as string),
			publishedAt: typeof r.published_at === 'string' ? r.published_at : null,
			url: typeof r.html_url === 'string' ? r.html_url : '',
			body: typeof r.body === 'string' ? r.body : ''
		}));
}

interface Entry {
	at: number;
	releases: Promise<Release[]>;
}

/**
 * Per-repo cache of release lists. Successes live for the TTL; failures are not cached.
 * Concurrent callers share one in-flight request.
 */
export function createReleaseCache(ttlMs = RELEASE_CACHE_TTL_MS, now: () => number = Date.now) {
	const entries = new Map<string, Entry>();
	return {
		get(repo: string, load: () => Promise<Release[]>): Promise<Release[]> {
			const hit = entries.get(repo);
			if (hit && now() - hit.at < ttlMs) return hit.releases;
			const releases = load();
			const entry = { at: now(), releases };
			entries.set(repo, entry);
			releases.catch(() => {
				if (entries.get(repo) === entry) entries.delete(repo);
			});
			return releases;
		},
		clear(): void {
			entries.clear();
		}
	};
}

export type ReleaseCache = ReturnType<typeof createReleaseCache>;

export const releaseCache = createReleaseCache();

export const cachedReleases = (repo: string, opts?: FetchReleasesOptions) =>
	releaseCache.get(repo, () => fetchReleases(repo, opts));

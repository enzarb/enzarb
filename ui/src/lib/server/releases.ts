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

export const RELEASES_REPO = 'enzarb/enzarb';

/** Monorepo release page for a full tag such as `ui/v1.2.3`. */
export const releaseTagUrl = (tag: string) =>
	`https://github.com/${RELEASES_REPO}/releases/tag/${encodeURIComponent(tag).replaceAll('%2F', '/')}`;

/**
 * First page (100) of the enzarb monorepo's published releases, newest first, with full tags
 * (`<dir>/vX.Y.Z`). Throws on any failure.
 */
export async function fetchReleases({
	fetch: doFetch = fetch,
	token,
	signal
}: FetchReleasesOptions = {}): Promise<Release[]> {
	const headers: Record<string, string> = {
		Accept: 'application/vnd.github+json',
		'User-Agent': 'enzarb-ui'
	};
	if (token) headers.Authorization = `Bearer ${token}`;
	const res = await doFetch(`https://api.github.com/repos/${RELEASES_REPO}/releases?per_page=100`, {
		headers,
		signal: signal ?? AbortSignal.timeout(RELEASE_FETCH_TIMEOUT_MS)
	});
	if (!res.ok) throw new Error(`GitHub releases for ${RELEASES_REPO}: HTTP ${res.status}`);
	const json: unknown = await res.json();
	if (!Array.isArray(json))
		throw new Error(`GitHub releases for ${RELEASES_REPO}: unexpected response`);
	return (json as GithubRelease[])
		.filter((r) => !r.draft && typeof r.tag_name === 'string')
		.map((r) => ({
			tag: r.tag_name as string,
			name: typeof r.name === 'string' && r.name ? r.name : (r.tag_name as string),
			publishedAt: typeof r.published_at === 'string' ? r.published_at : null,
			url: typeof r.html_url === 'string' ? r.html_url : releaseTagUrl(r.tag_name as string),
			body: typeof r.body === 'string' ? r.body : ''
		}));
}

/**
 * The releases of one component: tags starting with `<dir>/v`, with the `<dir>/` prefix stripped
 * from `tag` (so it compares against the deployed version) and `url` pointing at the full tag.
 */
export function componentReleases(dir: string, all: readonly Release[]): Release[] {
	const prefix = `${dir}/`;
	return all
		.filter((r) => r.tag.startsWith(`${prefix}v`))
		.map((r) => ({
			...r,
			tag: r.tag.slice(prefix.length),
			url: releaseTagUrl(r.tag)
		}));
}

interface Entry {
	at: number;
	releases: Promise<Release[]>;
}

/**
 * Cache of release lists, keyed by repo. Successes live for the TTL; failures are not cached.
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

/** One component's releases, from a single cached fetch of the monorepo's release list. */
export const cachedReleases = async (dir: string, opts?: FetchReleasesOptions) =>
	componentReleases(dir, await releaseCache.get(RELEASES_REPO, () => fetchReleases(opts)));

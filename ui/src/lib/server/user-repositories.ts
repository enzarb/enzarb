import { GITHUB_API as API, githubHeaders, signAppJwt } from './github-app.ts';

/**
 * Live list of the repositories a user may put in a Project: every repository an installation of
 * the enzarb GitHub App can access (`GET /app/installations`, then `GET /installation/repositories`
 * with each installation's token) where the user has write, maintain or admin
 * (`GET /repos/{o}/{r}/collaborators/{login}/permission`). No user token is used or stored.
 *
 * The list is cached per user for `LIST_TTL_MS` purely for display. Authorization when a
 * repository is added to a Project never reads it: `check` asks GitHub again for that repository.
 */

const MAX_PAGES = 50;
/** Overall budget for one list or check (all requests). */
export const LIST_TIMEOUT_MS = 15_000;
/** How long a user's list is shown from memory before GitHub is asked again. */
export const LIST_TTL_MS = 60_000;
/** Permission checks in flight at once for one list. */
const CONCURRENCY = 8;
/** Installation tokens are refreshed this long before they expire. */
const TOKEN_MARGIN_MS = 5 * 60_000;
/** Roles that may put a repository into a Project. */
const WRITE_ROLES = new Set(['admin', 'maintain', 'write']);

export interface UserRepository {
	/** Repository key `github:<owner>/<repo>`, lowercase. */
	repo: string;
	installationId: number;
	accountLogin: string;
	private: boolean;
	defaultBranch: string;
}

export interface UserRepositoryList {
	/** Ordered by key. */
	repositories: UserRepository[];
	/** When GitHub was asked. */
	fetchedAt: Date;
}

type FetchFn = typeof fetch;

export interface RepositoryAccessOptions {
	appId?: string;
	privateKey?: string;
	fetch?: FetchFn;
	now?: () => number;
	ttlMs?: number;
	timeoutMs?: number;
}

export interface RepositoryAccess {
	/** Whether the App credentials are configured (otherwise every list is empty). */
	readonly configured: boolean;
	/** The user's repositories; `refresh` skips the display cache. Throws when GitHub fails. */
	list(login: string, opts?: { refresh?: boolean }): Promise<UserRepositoryList>;
	/**
	 * Live authorization for one repository key: the repository when the App can access it and
	 * `login` has write, maintain or admin right now; null otherwise (including any GitHub failure).
	 */
	check(login: string, key: string): Promise<UserRepository | null>;
}

interface RawInstallation {
	id: number;
	account: { login: string } | null;
	suspended_at?: string | null;
}
interface RawRepository {
	full_name: string;
	private: boolean;
	default_branch: string;
	archived?: boolean;
}

function nextLink(res: Response): string | null {
	for (const part of (res.headers.get('link') ?? '').split(',')) {
		const m = /<([^>]+)>\s*;\s*rel="next"/.exec(part);
		if (m) return m[1];
	}
	return null;
}

const enc = encodeURIComponent;

/** `github:owner/repo` -> parts; null for anything else. */
function parseKey(key: string): { owner: string; repo: string } | null {
	const m = /^github:([\w.-]+)\/([\w.-]+)$/.exec(key);
	return m ? { owner: m[1], repo: m[2] } : null;
}

export function createRepositoryAccess({
	appId,
	privateKey,
	fetch: doFetch = fetch,
	now = Date.now,
	ttlMs = LIST_TTL_MS,
	timeoutMs = LIST_TIMEOUT_MS
}: RepositoryAccessOptions): RepositoryAccess {
	const tokens = new Map<number, { token: string; refreshAt: number }>();
	const cache = new Map<string, { value: UserRepositoryList; until: number }>();
	const inflight = new Map<string, Promise<UserRepositoryList>>();

	const call = (url: string, token: string, signal: AbortSignal, init: RequestInit = {}) => {
		// Never send a token anywhere but the GitHub API.
		if (!url.startsWith(`${API}/`)) throw new Error('unexpected GitHub URL');
		return doFetch(url, { ...init, headers: githubHeaders(token), signal, redirect: 'error' });
	};

	async function paginate<T>(
		token: string,
		path: string,
		pick: (body: unknown) => T[],
		signal: AbortSignal
	): Promise<T[]> {
		const out: T[] = [];
		let url: string | null = `${API}${path}${path.includes('?') ? '&' : '?'}per_page=100`;
		for (let page = 0; url; page++) {
			if (page >= MAX_PAGES) throw new Error(`GitHub ${path} has more than ${MAX_PAGES} pages`);
			const res: Response = await call(url, token, signal);
			if (!res.ok) throw new Error(`GitHub ${path} returned ${res.status}`);
			out.push(...pick(await res.json()));
			url = nextLink(res);
		}
		return out;
	}

	const appJwt = () => signAppJwt(appId!, privateKey!, now());

	async function installationToken(id: number, signal: AbortSignal): Promise<string> {
		const hit = tokens.get(id);
		if (hit && now() < hit.refreshAt) return hit.token;
		const res = await call(`${API}/app/installations/${id}/access_tokens`, await appJwt(), signal, {
			method: 'POST',
			body: JSON.stringify({ permissions: { metadata: 'read' } })
		});
		if (!res.ok) throw new Error(`GitHub installation token returned ${res.status}`);
		const body = (await res.json()) as { token?: unknown; expires_at?: unknown };
		if (typeof body.token !== 'string' || !body.token) throw new Error('no installation token');
		const expires = typeof body.expires_at === 'string' ? Date.parse(body.expires_at) : NaN;
		const end = Number.isFinite(expires) ? expires : now() + 60 * 60_000;
		tokens.set(id, { token: body.token, refreshAt: end - TOKEN_MARGIN_MS });
		return body.token;
	}

	/** The user's role on a repository (`none` on 404). Throws on other failures. */
	async function role(
		token: string,
		owner: string,
		repo: string,
		login: string,
		signal: AbortSignal
	): Promise<string> {
		const res = await call(
			`${API}/repos/${enc(owner)}/${enc(repo)}/collaborators/${enc(login)}/permission`,
			token,
			signal
		);
		if (res.status === 404) return 'none';
		if (!res.ok) throw new Error(`GitHub permission check returned ${res.status}`);
		const body = (await res.json()) as { permission?: string; role_name?: string };
		// `role_name` distinguishes maintain from write; `permission` folds it into write.
		return body.role_name ?? body.permission ?? 'none';
	}

	const toRepository = (r: RawRepository, inst: number, account: string): UserRepository => ({
		repo: `github:${r.full_name.toLowerCase()}`,
		installationId: inst,
		accountLogin: account,
		private: r.private,
		defaultBranch: r.default_branch
	});

	async function fetchList(login: string): Promise<UserRepositoryList> {
		const fetchedAt = new Date(now());
		if (!appId || !privateKey) return { repositories: [], fetchedAt };
		const signal = AbortSignal.timeout(timeoutMs);
		const all = await paginate<RawInstallation>(
			await appJwt(),
			'/app/installations',
			(b) => (Array.isArray(b) ? (b as RawInstallation[]) : []),
			signal
		);
		// Suspended installations grant nothing.
		const candidates: { token: string; repo: RawRepository; inst: RawInstallation }[] = [];
		for (const inst of all.filter((i) => !i.suspended_at)) {
			const token = await installationToken(inst.id, signal);
			const repos = await paginate<RawRepository>(
				token,
				'/installation/repositories',
				(b) => (b as { repositories?: RawRepository[] }).repositories ?? [],
				signal
			);
			for (const repo of repos) if (!repo.archived) candidates.push({ token, repo, inst });
		}
		const allowed: (UserRepository | null)[] = new Array(candidates.length).fill(null);
		let next = 0;
		const worker = async () => {
			while (next < candidates.length) {
				const idx = next++;
				const { token, repo, inst } = candidates[idx];
				const [owner, name] = repo.full_name.split('/');
				if (WRITE_ROLES.has(await role(token, owner, name, login, signal)))
					allowed[idx] = toRepository(repo, inst.id, inst.account?.login ?? owner);
			}
		};
		await Promise.all(Array.from({ length: Math.min(CONCURRENCY, candidates.length) }, worker));
		const out = new Map<string, UserRepository>();
		for (const r of allowed) if (r) out.set(r.repo, r);
		return {
			repositories: [...out.values()].sort((a, b) => a.repo.localeCompare(b.repo)),
			fetchedAt
		};
	}

	return {
		configured: !!appId && !!privateKey,
		list(login, { refresh = false } = {}) {
			const key = login.toLowerCase();
			const hit = cache.get(key);
			if (!refresh && hit && now() < hit.until) return Promise.resolve(hit.value);
			let p = inflight.get(key);
			if (!p) {
				p = fetchList(login)
					.then((value) => {
						cache.set(key, { value, until: now() + ttlMs });
						return value;
					})
					.finally(() => inflight.delete(key));
				inflight.set(key, p);
			}
			return p;
		},
		async check(login, key) {
			const parts = parseKey(key);
			if (!parts || !appId || !privateKey) return null;
			try {
				const signal = AbortSignal.timeout(timeoutMs);
				const base = `${API}/repos/${enc(parts.owner)}/${enc(parts.repo)}`;
				const instRes = await call(`${base}/installation`, await appJwt(), signal);
				if (!instRes.ok) return null;
				const inst = (await instRes.json()) as RawInstallation;
				if (typeof inst.id !== 'number' || inst.suspended_at) return null;
				const token = await installationToken(inst.id, signal);
				const repoRes = await call(base, token, signal);
				if (!repoRes.ok) return null;
				const repo = (await repoRes.json()) as RawRepository;
				const found = toRepository(repo, inst.id, inst.account?.login ?? parts.owner);
				if (found.repo !== key.toLowerCase() || repo.archived) return null;
				const r = await role(token, parts.owner, parts.repo, login, signal);
				return WRITE_ROLES.has(r) ? found : null;
			} catch {
				// Deny on any failure; the message could carry key material.
				return null;
			}
		}
	};
}

export function installUrl(slug: string): string {
	return `https://github.com/apps/${slug}/installations/new`;
}

export interface AccountInstallation {
	accountLogin: string;
	installationId: number;
	repositoryCount: number;
}

/** Groups repositories by installation account, ordered by login. */
export function groupByAccount(repos: UserRepository[]): AccountInstallation[] {
	const groups = new Map<string, AccountInstallation>();
	for (const r of repos) {
		const key = `${r.accountLogin}\0${r.installationId}`;
		const g = groups.get(key);
		if (g) g.repositoryCount++;
		else
			groups.set(key, {
				accountLogin: r.accountLogin,
				installationId: r.installationId,
				repositoryCount: 1
			});
	}
	return [...groups.values()].sort((a, b) => a.accountLogin.localeCompare(b.accountLogin));
}

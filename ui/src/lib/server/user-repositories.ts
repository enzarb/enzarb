import type { Sql } from './db.ts';

const API = 'https://api.github.com';
const MAX_PAGES = 50;
/** Overall budget for one snapshot fetch (all requests). */
export const SYNC_TIMEOUT_MS = 10_000;
const CONCURRENCY = 4;

export interface UserRepository {
	/** Repository key `github:<owner>/<repo>`, lowercase. */
	repo: string;
	installationId: number;
	accountLogin: string;
	private: boolean;
	defaultBranch: string;
}

type FetchFn = typeof fetch;

function headers(token: string): Record<string, string> {
	return {
		Authorization: `Bearer ${token}`,
		Accept: 'application/vnd.github+json',
		'X-GitHub-Api-Version': '2022-11-28',
		'User-Agent': 'enzarb-ui'
	};
}

function nextLink(res: Response): string | null {
	for (const part of (res.headers.get('link') ?? '').split(',')) {
		const m = /<([^>]+)>\s*;\s*rel="next"/.exec(part);
		if (m) return m[1];
	}
	return null;
}

/** Follows `Link: rel="next"` and collects `pick(body)` from every page. */
async function paginate<T>(
	token: string,
	path: string,
	pick: (body: unknown) => T[],
	fetchFn: FetchFn,
	signal: AbortSignal,
	opts: { notFoundIsEmpty?: boolean } = {}
): Promise<T[]> {
	const out: T[] = [];
	let url: string | null = `${API}${path}${path.includes('?') ? '&' : '?'}per_page=100`;
	for (let page = 0; url; page++) {
		if (page >= MAX_PAGES) throw new Error(`GitHub ${path} has more than ${MAX_PAGES} pages`);
		// Never send the token anywhere but the GitHub API.
		if (!url.startsWith(`${API}/`)) throw new Error('GitHub returned an unexpected pagination URL');
		const res: Response = await fetchFn(url, {
			headers: headers(token),
			signal,
			redirect: 'error'
		});
		if (res.status === 404 && opts.notFoundIsEmpty) return out;
		if (!res.ok) throw new Error(`GitHub ${path} returned ${res.status}`);
		out.push(...pick(await res.json()));
		url = nextLink(res);
	}
	return out;
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
}

/**
 * The repositories of every installation of this App that the token's user can access
 * (`GET /user/installations`, then `GET /user/installations/{id}/repositories`).
 * Throws on any API failure so callers keep the previous snapshot rather than a partial one.
 */
export async function fetchUserRepositories(
	token: string,
	fetchFn: FetchFn = fetch,
	timeoutMs = SYNC_TIMEOUT_MS
): Promise<UserRepository[]> {
	const signal = AbortSignal.timeout(timeoutMs);
	const all = await paginate<RawInstallation>(
		token,
		'/user/installations',
		(b) => (b as { installations?: RawInstallation[] }).installations ?? [],
		fetchFn,
		signal
	);
	// Suspended installations grant nothing.
	const installations = all.filter((i) => !i.suspended_at);
	const perInstallation: UserRepository[][] = new Array(installations.length);
	let next = 0;
	const worker = async () => {
		while (next < installations.length) {
			const idx = next++;
			const inst = installations[idx];
			// A 404 means this installation has no repositories for the user; anything else is fatal.
			const repos = await paginate<RawRepository>(
				token,
				`/user/installations/${inst.id}/repositories`,
				(b) => (b as { repositories?: RawRepository[] }).repositories ?? [],
				fetchFn,
				signal,
				{ notFoundIsEmpty: true }
			);
			perInstallation[idx] = repos.map((r) => ({
				repo: `github:${r.full_name.toLowerCase()}`,
				installationId: inst.id,
				accountLogin: inst.account?.login ?? r.full_name.split('/')[0],
				private: r.private,
				defaultBranch: r.default_branch
			}));
		}
	};
	await Promise.all(Array.from({ length: Math.min(CONCURRENCY, installations.length) }, worker));
	const out = new Map<string, UserRepository>();
	for (const r of perInstallation.flat()) out.set(r.repo, r);
	return [...out.values()];
}

/** Replaces the user's snapshot atomically. */
export async function replaceUserRepositories(
	sql: Sql,
	userId: number,
	repos: UserRepository[]
): Promise<void> {
	await sql.begin(async (tx) => {
		await tx`DELETE FROM app.user_repositories WHERE user_id = ${userId}`;
		if (repos.length === 0) return;
		const rows = repos.map((r) => ({
			user_id: userId,
			repo: r.repo,
			installation_id: r.installationId,
			account_login: r.accountLogin,
			private: r.private,
			default_branch: r.defaultBranch
		}));
		await tx`INSERT INTO app.user_repositories ${tx(
			rows,
			'user_id',
			'repo',
			'installation_id',
			'account_login',
			'private',
			'default_branch'
		)}`;
	});
}

/**
 * Sign-in hook: fetch with the user-to-server token and store the snapshot. The token is only
 * used here and never stored. A GitHub (or database) failure is logged and the previous snapshot
 * is kept, so sign-in is never blocked.
 */
export async function syncUserRepositories(
	sql: Sql,
	userId: number,
	token: string,
	fetchFn: FetchFn = fetch
): Promise<boolean> {
	try {
		await replaceUserRepositories(sql, userId, await fetchUserRepositories(token, fetchFn));
		return true;
	} catch (e) {
		console.error('could not refresh the user repository snapshot:', (e as Error).message);
		return false;
	}
}

/**
 * The current user's repositories, ordered by key. This snapshot is a UI hint only (what to
 * offer, whether to show the install notice); it is never an authorization source.
 */
export async function listUserRepositories(sql: Sql, userId: number): Promise<UserRepository[]> {
	const rows = await sql<
		{
			repo: string;
			installation_id: string;
			account_login: string;
			private: boolean;
			default_branch: string;
		}[]
	>`SELECT repo, installation_id, account_login, private, default_branch
		FROM app.user_repositories WHERE user_id = ${userId} ORDER BY repo`;
	return rows.map((r) => ({
		repo: r.repo,
		installationId: Number(r.installation_id),
		accountLogin: r.account_login,
		private: r.private,
		defaultBranch: r.default_branch
	}));
}

export function installUrl(slug: string): string {
	return `https://github.com/apps/${slug}/installations/new`;
}

/** Whether the snapshot holds at least one repository (drives the install-the-App banner). */
export async function hasUserRepositories(sql: Sql, userId: number): Promise<boolean> {
	const [row] = await sql<
		{ found: boolean }[]
	>`SELECT EXISTS (SELECT 1 FROM app.user_repositories WHERE user_id = ${userId}) AS found`;
	return row.found;
}

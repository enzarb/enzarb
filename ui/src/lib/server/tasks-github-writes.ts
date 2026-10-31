import type { RepoPermissions } from './github-app.ts';
import { githubFetch } from './github-app.ts';

/**
 * The GitHub writes behind Approve & merge, done as the enzarb App with an installation token
 * (no user token exists, so nothing here ever acts "as" the user). Each call resolves to
 * `{ok:true}` or `{ok:false, status, message}` and never throws.
 */

export type WriteResult = { ok: true } | { ok: false; status: number; message: string };

export interface GithubWrites {
	/** An approving review by the App; `body` must say who approved. */
	approve(repo: string, n: number, body: string, sha: string): Promise<WriteResult>;
	/** A plain comment, the fallback when the App may not approve (its own pull request). */
	comment(repo: string, n: number, body: string): Promise<WriteResult>;
	/** Squash-merge, guarded by the head `sha` that was checked. */
	squashMerge(repo: string, n: number, input: { title: string; sha: string }): Promise<WriteResult>;
}

export interface GithubWritesDeps {
	token: (repo: string, perms: RepoPermissions) => Promise<string | null>;
	invalidate: (repo: string, perms: RepoPermissions) => void;
	fetch?: typeof fetch;
}

const PERMS: RepoPermissions = { pull_requests: 'write', contents: 'write' };
const enc = (repo: string) => repo.split('/').map(encodeURIComponent).join('/');

export function createGithubWrites({
	token,
	invalidate,
	fetch: doFetch = fetch
}: GithubWritesDeps): GithubWrites {
	async function send(
		repo: string,
		method: string,
		path: string,
		body: unknown
	): Promise<WriteResult> {
		const t = await token(repo, PERMS);
		if (!t) return { ok: false, status: 0, message: 'The enzarb GitHub App cannot write to it.' };
		let res: Response;
		try {
			res = await githubFetch(`/repos/${enc(repo)}${path}`, {
				method,
				token: t,
				fetch: doFetch,
				redirect: 'error',
				body: JSON.stringify(body),
				signal: AbortSignal.timeout(15_000)
			});
		} catch {
			return { ok: false, status: 0, message: 'GitHub could not be reached.' };
		}
		if (res.status === 401) invalidate(repo, PERMS);
		if (res.ok) return { ok: true };
		let message = `GitHub answered ${res.status}.`;
		try {
			const j = (await res.json()) as { message?: unknown };
			if (typeof j.message === 'string' && j.message) message = j.message;
		} catch {
			// keep the status text
		}
		return { ok: false, status: res.status, message };
	}

	return {
		approve: (repo, n, body, sha) =>
			send(repo, 'POST', `/pulls/${n}/reviews`, { event: 'APPROVE', body, commit_id: sha }),
		comment: (repo, n, body) => send(repo, 'POST', `/issues/${n}/comments`, { body }),
		squashMerge: (repo, n, { title, sha }) =>
			send(repo, 'PUT', `/pulls/${n}/merge`, {
				merge_method: 'squash',
				commit_title: title,
				sha
			})
	};
}

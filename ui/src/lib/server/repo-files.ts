import { githubFetch } from './github-app.ts';

/**
 * Read and commit single files on a branch through the GitHub contents API with an installation
 * token (ADR-0029 §4: designer saves are direct commits by the enzarb App). Optimistic concurrency
 * uses the file's blob sha: a commit names the sha it replaces, and GitHub refuses it when the
 * branch has moved on.
 */

const TIMEOUT_MS = 15_000;
/** The contents API returns content inline only up to 1 MB; config files are far smaller. */
export const MAX_FILE_BYTES = 512 * 1024;

const enc = encodeURIComponent;
const contentsPath = (repo: string, path: string) => {
	const [owner, name] = repo.split('/');
	return `/repos/${enc(owner)}/${enc(name)}/contents/${path.split('/').map(enc).join('/')}`;
};

export interface RepoFile {
	text: string;
	/** Blob sha, the concurrency token for the next commit. */
	sha: string;
}

export type ReadResult =
	{ ok: true; file: RepoFile | null } | { ok: false; status: number; message: string };

/** The file at `ref`, `file: null` when it does not exist. */
export async function readRepoFile(opts: {
	repo: string;
	path: string;
	ref: string;
	token: string;
	fetch?: typeof fetch;
}): Promise<ReadResult> {
	const res = await githubFetch(`${contentsPath(opts.repo, opts.path)}?ref=${enc(opts.ref)}`, {
		token: opts.token,
		fetch: opts.fetch,
		redirect: 'error',
		signal: AbortSignal.timeout(TIMEOUT_MS)
	});
	if (res.status === 404) return { ok: true, file: null };
	if (!res.ok) return { ok: false, status: res.status, message: `GitHub returned ${res.status}` };
	const body = (await res.json()) as {
		type?: unknown;
		encoding?: unknown;
		content?: unknown;
		sha?: unknown;
		size?: unknown;
	};
	if (body.type !== 'file' || typeof body.sha !== 'string')
		return { ok: false, status: 422, message: `${opts.path} is not a file` };
	if (typeof body.size === 'number' && body.size > MAX_FILE_BYTES)
		return {
			ok: false,
			status: 413,
			message: `${opts.path} is larger than ${MAX_FILE_BYTES} bytes`
		};
	if (body.encoding !== 'base64' || typeof body.content !== 'string')
		return { ok: false, status: 422, message: `${opts.path} has no inline content` };
	const text = Buffer.from(body.content, 'base64').toString('utf8');
	return { ok: true, file: { text, sha: body.sha } };
}

export type CommitResult =
	| { ok: true; sha: string; commitSha: string; commitUrl: string | null }
	/** `conflict`: the branch moved (sha mismatch); `forbidden`: no write (permission, protection). */
	| { ok: false; reason: 'conflict' | 'forbidden' | 'failed'; status: number; message: string };

/**
 * Commit `text` to `path` on `branch`. `sha` is the blob being replaced, null to create the file.
 * The commit is authored by the App; `message` should carry a `Co-authored-by` trailer.
 */
export async function commitRepoFile(opts: {
	repo: string;
	path: string;
	branch: string;
	text: string;
	sha: string | null;
	message: string;
	token: string;
	fetch?: typeof fetch;
}): Promise<CommitResult> {
	const res = await githubFetch(contentsPath(opts.repo, opts.path), {
		method: 'PUT',
		token: opts.token,
		fetch: opts.fetch,
		redirect: 'error',
		signal: AbortSignal.timeout(TIMEOUT_MS),
		body: JSON.stringify({
			message: opts.message,
			content: Buffer.from(opts.text, 'utf8').toString('base64'),
			branch: opts.branch,
			...(opts.sha ? { sha: opts.sha } : {})
		})
	});
	if (res.ok) {
		const body = (await res.json()) as {
			content?: { sha?: unknown };
			commit?: { sha?: unknown; html_url?: unknown };
		};
		return {
			ok: true,
			sha: typeof body.content?.sha === 'string' ? body.content.sha : '',
			commitSha: typeof body.commit?.sha === 'string' ? body.commit.sha : '',
			commitUrl: typeof body.commit?.html_url === 'string' ? body.commit.html_url : null
		};
	}
	// 409: sha does not match the branch head; 422 "sha wasn't supplied": someone created the file.
	if (res.status === 409 || res.status === 422)
		return { ok: false, reason: 'conflict', status: res.status, message: 'the file changed' };
	if (res.status === 403 || res.status === 404)
		return {
			ok: false,
			reason: 'forbidden',
			status: res.status,
			message: 'the commit was refused'
		};
	return {
		ok: false,
		reason: 'failed',
		status: res.status,
		message: `GitHub returned ${res.status}`
	};
}

/** `Co-authored-by` trailer for a GitHub user (noreply address, so no email is looked up). */
export function coAuthorTrailer(user: { login: string; githubId: number }): string {
	return `Co-authored-by: ${user.login} <${user.githubId}+${user.login}@users.noreply.github.com>`;
}

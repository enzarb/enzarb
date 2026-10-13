import {
	sameOwner,
	validateDescription,
	validateRepoName,
	type RepoResult
} from '#lib/repo-names.ts';

export { sameOwner, validateDescription, validateRepoName };

const API = 'https://api.github.com';
export const CREATE_TIMEOUT_MS = 15_000;

type FetchFn = typeof fetch;

/** A failed GitHub write. `message` is safe to show; `code` is the fixed result code. */
export class RepoCreateError extends Error {
	constructor(
		readonly code: Exclude<RepoResult, 'created' | 'identity-mismatch' | 'owner-not-allowed'>,
		message: string,
		readonly status?: number
	) {
		super(message);
		this.name = 'RepoCreateError';
	}
}

export interface CreatedRepository {
	id: number;
	/** `owner/name` as GitHub spells it. */
	fullName: string;
	htmlUrl: string;
	private: boolean;
	defaultBranch: string;
}

export interface CreateRepositoryInput {
	owner: string;
	/** Login of the token's user: decides between `/user/repos` and `/orgs/{org}/repos`. */
	selfLogin: string;
	name: string;
	private: boolean;
	description: string;
}

function headers(token: string): Record<string, string> {
	return {
		Authorization: `Bearer ${token}`,
		Accept: 'application/vnd.github+json',
		'Content-Type': 'application/json',
		'X-GitHub-Api-Version': '2022-11-28',
		'User-Agent': 'enzarb-ui'
	};
}

async function githubMessage(res: Response): Promise<string> {
	try {
		const body = (await res.json()) as {
			message?: unknown;
			errors?: { message?: unknown; code?: unknown }[];
		};
		const parts = [body.message, ...(body.errors ?? []).map((e) => e.message)].filter(
			(m): m is string => typeof m === 'string' && m.length > 0
		);
		return parts.join('; ').slice(0, 300);
	} catch {
		return '';
	}
}

async function send(
	token: string,
	method: string,
	path: string,
	body: unknown,
	fetchFn: FetchFn,
	timeoutMs: number
): Promise<Response> {
	try {
		return await fetchFn(`${API}${path}`, {
			method,
			headers: headers(token),
			body: body === undefined ? undefined : JSON.stringify(body),
			redirect: 'error',
			signal: AbortSignal.timeout(timeoutMs)
		});
	} catch {
		// Timeout, network failure or a redirect: the outcome is unknown, the repo may exist.
		throw new RepoCreateError(
			'timeout',
			'GitHub did not answer in time. The repository may exist: use Refresh repositories.'
		);
	}
}

/**
 * Creates one repository with a user-to-server token (never the installation token):
 * `POST /user/repos` for the user's own account, `POST /orgs/{org}/repos` otherwise. Always
 * `auto_init` so the default branch exists.
 */
export async function createRepository(
	token: string,
	input: CreateRepositoryInput,
	fetchFn: FetchFn = fetch,
	timeoutMs = CREATE_TIMEOUT_MS
): Promise<CreatedRepository> {
	const problem = validateRepoName(input.name) ?? validateDescription(input.description);
	if (problem) throw new RepoCreateError('invalid', problem);
	const path = sameOwner(input.owner, input.selfLogin)
		? '/user/repos'
		: `/orgs/${encodeURIComponent(input.owner)}/repos`;
	const res = await send(
		token,
		'POST',
		path,
		{
			name: input.name,
			private: input.private,
			description: input.description,
			auto_init: true
		},
		fetchFn,
		timeoutMs
	);
	if (res.status === 201 || res.status === 200) {
		const r = (await res.json()) as {
			id: number;
			full_name: string;
			html_url: string;
			private: boolean;
			default_branch?: string;
		};
		return {
			id: r.id,
			fullName: r.full_name,
			htmlUrl: r.html_url,
			private: r.private,
			defaultBranch: r.default_branch ?? 'main'
		};
	}
	const detail = await githubMessage(res);
	const key = `${input.owner}/${input.name}`;
	if (res.status === 403 || res.status === 404)
		throw new RepoCreateError(
			'forbidden',
			`GitHub refused (${res.status}${detail ? `: ${detail}` : ''}). Either the GitHub App lacks the Administration: write permission, user tokens cannot create repositories for this account type, or you lack the right to create repositories in ${input.owner}.`,
			res.status
		);
	if (res.status === 422) {
		if (/name already exists/i.test(detail))
			throw new RepoCreateError(
				'name-taken',
				`${key} already exists. Select it in the list below if you created it earlier.`,
				422
			);
		throw new RepoCreateError('invalid', `GitHub rejected the repository (422: ${detail}).`, 422);
	}
	throw new RepoCreateError(
		'failed',
		`GitHub could not create the repository (${res.status}${detail ? `: ${detail}` : ''}).`,
		res.status
	);
}

/** Adds a repository to an installation that covers "selected repositories" only. */
export async function addRepoToInstallation(
	token: string,
	installationId: number,
	repoId: number,
	fetchFn: FetchFn = fetch,
	timeoutMs = CREATE_TIMEOUT_MS
): Promise<void> {
	const res = await send(
		token,
		'PUT',
		`/user/installations/${installationId}/repositories/${repoId}`,
		undefined,
		fetchFn,
		timeoutMs
	);
	if (res.ok) return; // 204; 304 would mean already covered
	throw new RepoCreateError(
		'created-not-installed',
		`GitHub refused to add the repository to the installation (${res.status}).`,
		res.status
	);
}

/** Whether an installation covers all repositories of its account (`false`: selected only). */
export async function installationCoversAll(
	token: string,
	installationId: number,
	fetchFn: FetchFn = fetch,
	timeoutMs = CREATE_TIMEOUT_MS
): Promise<boolean | null> {
	try {
		for (let page = 1; page <= 10; page++) {
			const res = await fetchFn(`${API}/user/installations?per_page=100&page=${page}`, {
				headers: headers(token),
				redirect: 'error',
				signal: AbortSignal.timeout(timeoutMs)
			});
			if (!res.ok) return null;
			const body = (await res.json()) as {
				installations?: { id: number; repository_selection?: string }[];
			};
			const list = body.installations ?? [];
			const found = list.find((i) => i.id === installationId);
			if (found) return found.repository_selection === 'all';
			if (list.length < 100) return null;
		}
	} catch {
		return null;
	}
	return null;
}

/**
 * Reading and proposing changes to `.enzarb/workflows/*.yaml` and `.enzarb/prompts/*.md` of a
 * project repository, server-side with a GitHub App installation token (never in the browser).
 * A change is always a new branch `enzarb/workflow-<slug>-<short>` plus a pull request; nothing is
 * ever committed to the default branch.
 */
import { randomBytes } from 'node:crypto';
import { GITHUB_API, githubHeaders } from './github-app.ts';
import { isPromptPath, PROMPT_DIR, WORKFLOW_DIR } from '#lib/workflow/definition.ts';

const TIMEOUT_MS = 15_000;
/** Largest file read or written, in bytes. */
export const MAX_FILE_BYTES = 256 * 1024;
export const BRANCH_PREFIX = 'enzarb/workflow-';

export class RepoError extends Error {
	constructor(
		message: string,
		readonly status?: number
	) {
		super(message);
		this.name = 'RepoError';
	}
}

export interface DirEntry {
	name: string;
	path: string;
	type: string;
}

export interface FileContent {
	path: string;
	text: string;
	sha: string;
}

export interface ChangedFile {
	path: string;
	text: string;
}

/** The GitHub calls this module needs; a fake implements it in tests. */
export interface RepoApi {
	defaultBranch(): Promise<string>;
	listDir(path: string, ref: string): Promise<DirEntry[]>;
	getFile(path: string, ref: string): Promise<FileContent | null>;
	branchSha(branch: string): Promise<string | null>;
	createBranch(branch: string, sha: string): Promise<void>;
	/** One commit with every file on top of `branch`; returns the new commit sha. */
	commitFiles(branch: string, files: ChangedFile[], message: string): Promise<string>;
	openPullRequest(input: {
		head: string;
		base: string;
		title: string;
		body: string;
	}): Promise<{ number: number; url: string }>;
	/** `admin`, `maintain`, `write`, `triage`, `read` or `none`. */
	permission(login: string): Promise<string>;
}

const enc = encodeURIComponent;
const encPath = (p: string) => p.split('/').map(enc).join('/');

/** {@link RepoApi} over the GitHub REST API with an installation token for `owner/repo`. */
export function githubRepoApi(
	token: string,
	fullName: string,
	doFetch: typeof fetch = fetch
): RepoApi {
	const [owner, repo] = fullName.split('/');
	const base = `${GITHUB_API}/repos/${enc(owner)}/${enc(repo)}`;
	async function call(method: string, path: string, body?: unknown): Promise<Response> {
		try {
			return await doFetch(`${base}${path}`, {
				method,
				headers: githubHeaders(token),
				body: body === undefined ? undefined : JSON.stringify(body),
				redirect: 'error',
				signal: AbortSignal.timeout(TIMEOUT_MS)
			});
		} catch {
			throw new RepoError('GitHub did not answer in time.');
		}
	}
	async function ok<T>(res: Response, what: string): Promise<T> {
		if (res.ok) return (await res.json()) as T;
		let detail = '';
		try {
			const m = ((await res.json()) as { message?: unknown }).message;
			if (typeof m === 'string') detail = `: ${m.slice(0, 200)}`;
		} catch {
			// no body
		}
		throw new RepoError(`GitHub could not ${what} (${res.status}${detail}).`, res.status);
	}
	return {
		async defaultBranch() {
			return (await ok<{ default_branch: string }>(await call('GET', ''), 'read the repository'))
				.default_branch;
		},
		async listDir(path, ref) {
			const res = await call('GET', `/contents/${encPath(path)}?ref=${enc(ref)}`);
			if (res.status === 404) return [];
			const body = await ok<unknown>(res, `list ${path}`);
			return Array.isArray(body)
				? body.map((e: DirEntry) => ({ name: e.name, path: e.path, type: e.type }))
				: [];
		},
		async getFile(path, ref) {
			const res = await call('GET', `/contents/${encPath(path)}?ref=${enc(ref)}`);
			if (res.status === 404) return null;
			const body = await ok<{ type?: string; content?: string; sha: string; size?: number }>(
				res,
				`read ${path}`
			);
			if (body.type !== 'file' || typeof body.content !== 'string') return null;
			if ((body.size ?? 0) > MAX_FILE_BYTES) throw new RepoError(`${path} is too large.`);
			return { path, sha: body.sha, text: Buffer.from(body.content, 'base64').toString('utf8') };
		},
		async branchSha(branch) {
			const res = await call('GET', `/git/ref/heads/${encPath(branch)}`);
			if (res.status === 404) return null;
			return (await ok<{ object: { sha: string } }>(res, `read ${branch}`)).object.sha;
		},
		async createBranch(branch, sha) {
			await ok(
				await call('POST', '/git/refs', { ref: `refs/heads/${branch}`, sha }),
				`create the branch ${branch}`
			);
		},
		async commitFiles(branch, files, message) {
			const head = await this.branchSha(branch);
			if (!head) throw new RepoError(`The branch ${branch} does not exist.`);
			const commit = await ok<{ tree: { sha: string } }>(
				await call('GET', `/git/commits/${head}`),
				'read the branch head'
			);
			const tree = await ok<{ sha: string }>(
				await call('POST', '/git/trees', {
					base_tree: commit.tree.sha,
					tree: files.map((f) => ({ path: f.path, mode: '100644', type: 'blob', content: f.text }))
				}),
				'write the files'
			);
			const created = await ok<{ sha: string }>(
				await call('POST', '/git/commits', { message, tree: tree.sha, parents: [head] }),
				'commit'
			);
			await ok(
				await call('PATCH', `/git/refs/heads/${encPath(branch)}`, {
					sha: created.sha,
					force: false
				}),
				`update ${branch}`
			);
			return created.sha;
		},
		async openPullRequest({ head, base: baseBranch, title, body }) {
			const pr = await ok<{ number: number; html_url: string }>(
				await call('POST', '/pulls', { head, base: baseBranch, title, body }),
				'open the pull request'
			);
			return { number: pr.number, url: pr.html_url };
		},
		async permission(login) {
			const res = await call('GET', `/collaborators/${enc(login)}/permission`);
			if (res.status === 404) return 'none';
			const body = await ok<{ permission?: string; role_name?: string }>(res, 'check your access');
			// `role_name` distinguishes maintain from write; `permission` folds it into write.
			return body.role_name ?? body.permission ?? 'none';
		}
	};
}

// --- reading ---------------------------------------------------------------------------------

export interface RepoWorkflows {
	defaultBranch: string;
	/** `.enzarb/workflows/*.yaml`, sorted. */
	workflows: string[];
	/** `.enzarb/prompts/*.md`, sorted. */
	prompts: string[];
}

export async function listRepoWorkflows(api: RepoApi): Promise<RepoWorkflows> {
	const defaultBranch = await api.defaultBranch();
	const files = async (dir: string, re: RegExp) =>
		(await api.listDir(dir.replace(/\/$/, ''), defaultBranch))
			.filter((e) => e.type === 'file' && re.test(e.name))
			.map((e) => `${dir}${e.name}`)
			.sort();
	const [workflows, prompts] = await Promise.all([
		files(WORKFLOW_DIR, /^[A-Za-z0-9_-][A-Za-z0-9_.-]*\.ya?ml$/),
		files(PROMPT_DIR, /^[A-Za-z0-9_-][A-Za-z0-9_.-]*\.md$/)
	]);
	return { defaultBranch, workflows, prompts };
}

export function isWorkflowPath(path: string): boolean {
	return (
		/^\.enzarb\/workflows\/[A-Za-z0-9_-][A-Za-z0-9_.-]*\.ya?ml$/.test(path) && !path.includes('..')
	);
}

/** Only workflow definitions and prompt files are read or written through the editor. */
export function isEditablePath(path: string): boolean {
	return isWorkflowPath(path) || isPromptPath(path);
}

// --- proposing -------------------------------------------------------------------------------

const WRITE_ROLES = new Set(['admin', 'maintain', 'write']);

export async function requireWrite(api: RepoApi, login: string): Promise<void> {
	const role = await api.permission(login);
	if (!WRITE_ROLES.has(role))
		throw new RepoError('You need write access to this repository to propose a change.', 403);
}

/** `enzarb/workflow-<slug>-<short>`: slug from the workflow name, short random suffix. */
export function proposalBranch(
	workflowName: string,
	short = randomBytes(3).toString('hex')
): string {
	const slug =
		workflowName
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, '-')
			.replace(/^-+|-+$/g, '')
			.slice(0, 40) || 'workflow';
	return `${BRANCH_PREFIX}${slug}-${short}`;
}

function isProposalBranch(branch: string): boolean {
	return /^enzarb\/workflow-[a-z0-9-]{1,40}-[a-f0-9]{6}$/.test(branch);
}

export function checkFiles(files: ChangedFile[]): void {
	if (files.length === 0) throw new RepoError('Nothing to commit.');
	if (files.length > 20) throw new RepoError('At most 20 files per change.');
	const seen = new Set<string>();
	for (const f of files) {
		if (!isEditablePath(f.path))
			throw new RepoError(`${f.path} is not a workflow definition or prompt file.`);
		if (seen.has(f.path)) throw new RepoError(`${f.path} is listed twice.`);
		seen.add(f.path);
		if (Buffer.byteLength(f.text, 'utf8') > MAX_FILE_BYTES)
			throw new RepoError(`${f.path} is too large.`);
	}
}

/** Step 1: a new branch from the default branch head. Returns the branch and base. */
export async function createProposalBranch(
	api: RepoApi,
	login: string,
	workflowName: string
): Promise<{ branch: string; base: string }> {
	await requireWrite(api, login);
	const base = await api.defaultBranch();
	const sha = await api.branchSha(base);
	if (!sha) throw new RepoError(`The default branch ${base} has no head.`);
	const branch = proposalBranch(workflowName);
	await api.createBranch(branch, sha);
	return { branch, base };
}

/** Step 2: one commit with the changed files on the proposal branch (never the default branch). */
export async function commitProposal(
	api: RepoApi,
	login: string,
	branch: string,
	workflowName: string,
	files: ChangedFile[]
): Promise<string> {
	await requireWrite(api, login);
	if (!isProposalBranch(branch)) throw new RepoError('Not a workflow proposal branch.');
	if (branch === (await api.defaultBranch()))
		throw new RepoError('Refusing to commit to the default branch.');
	checkFiles(files);
	return api.commitFiles(branch, files, `chore(workflows): update ${workflowName} workflow`);
}

/** Step 3: the pull request from the proposal branch into the default branch. */
export async function openProposal(
	api: RepoApi,
	login: string,
	branch: string,
	workflowName: string,
	files: string[]
): Promise<{ number: number; url: string }> {
	await requireWrite(api, login);
	if (!isProposalBranch(branch)) throw new RepoError('Not a workflow proposal branch.');
	const base = await api.defaultBranch();
	const list = files.map((f) => `- \`${f}\``).join('\n');
	return api.openPullRequest({
		head: branch,
		base,
		title: `chore(workflows): update ${workflowName} workflow`,
		body: `Proposed by @${login} with the enzarb workflow editor.\n\nChanged files:\n${list}\n`
	});
}

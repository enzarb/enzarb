/**
 * Reading and proposing changes to `.enzarb/workflows/*.yaml` and `.enzarb/prompts/*.md` of a
 * project repository, server-side with a GitHub App installation token (never in the browser).
 * A change is always a new branch `enzarb/workflow-<slug>-<short>` plus a pull request; nothing is
 * ever committed to the default branch. The three steps are chained by a signed proposal token,
 * so a client cannot point a later step at another branch, repository or user.
 */
import { createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import { GITHUB_API, githubHeaders } from './github-app.ts';
import { isPromptPath, PROMPT_DIR, WORKFLOW_DIR } from '#lib/workflow/definition.ts';
import { WORKFLOW_NAME_RE } from '#lib/workflow/propose.ts';

const TIMEOUT_MS = 15_000;
/** Largest file read or written, in bytes. */
export const MAX_FILE_BYTES = 256 * 1024;
/** Largest total of one commit, in bytes. */
export const MAX_TOTAL_BYTES = 1024 * 1024;
export const MAX_FILES = 20;
export const BRANCH_PREFIX = 'enzarb/workflow-';
/** How long a proposal token is accepted. */
export const PROPOSAL_TTL_MS = 60 * 60_000;

/**
 * A failed repository operation. `message` is a fixed text safe to show; `detail` (GitHub's own
 * message) is only logged.
 */
export class RepoError extends Error {
	constructor(
		message: string,
		readonly status?: number,
		readonly detail?: string
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
	/** One commit with every file on top of `head` (the branch's current head); returns its sha. */
	commitFiles(branch: string, head: string, files: ChangedFile[], message: string): Promise<string>;
	/** Paths changed between `base` and `head` (`GET /compare`). */
	changedPaths(base: string, head: string): Promise<string[]>;
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
			if (typeof m === 'string') detail = m.slice(0, 300);
		} catch {
			// no body
		}
		throw new RepoError(`GitHub could not ${what}.`, res.status, `${res.status} ${detail}`);
	}
	async function branchSha(branch: string) {
		const res = await call('GET', `/git/ref/heads/${encPath(branch)}`);
		if (res.status === 404) return null;
		return (await ok<{ object: { sha: string } }>(res, 'read the branch')).object.sha;
	}
	return {
		async defaultBranch() {
			return (await ok<{ default_branch: string }>(await call('GET', ''), 'read the repository'))
				.default_branch;
		},
		async listDir(path, ref) {
			const res = await call('GET', `/contents/${encPath(path)}?ref=${enc(ref)}`);
			if (res.status === 404) return [];
			const body = await ok<unknown>(res, 'list the files');
			return Array.isArray(body)
				? body.map((e: DirEntry) => ({ name: e.name, path: e.path, type: e.type }))
				: [];
		},
		async getFile(path, ref) {
			const res = await call('GET', `/contents/${encPath(path)}?ref=${enc(ref)}`);
			if (res.status === 404) return null;
			const body = await ok<{ type?: string; content?: string; sha: string; size?: number }>(
				res,
				'read the file'
			);
			if (body.type !== 'file' || typeof body.content !== 'string') return null;
			if ((body.size ?? 0) > MAX_FILE_BYTES) throw new RepoError('The file is too large.');
			return { path, sha: body.sha, text: Buffer.from(body.content, 'base64').toString('utf8') };
		},
		branchSha,
		async createBranch(branch, sha) {
			await ok(
				await call('POST', '/git/refs', { ref: `refs/heads/${branch}`, sha }),
				'create the branch'
			);
		},
		async commitFiles(branch, head, files, message) {
			const commit = await ok<{ tree: { sha: string } }>(
				await call('GET', `/git/commits/${enc(head)}`),
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
			// Not forced: GitHub refuses if the branch moved since `head`.
			await ok(
				await call('PATCH', `/git/refs/heads/${encPath(branch)}`, {
					sha: created.sha,
					force: false
				}),
				'update the branch'
			);
			return created.sha;
		},
		async changedPaths(baseRef, headRef) {
			const body = await ok<{ files?: { filename: string }[] }>(
				await call('GET', `/compare/${enc(baseRef)}...${enc(headRef)}`),
				'compare the branch'
			);
			return (body.files ?? []).map((f) => f.filename);
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

// --- paths -----------------------------------------------------------------------------------

export function isWorkflowPath(path: string): boolean {
	return /^\.enzarb\/workflows\/[A-Za-z0-9_-]+\.ya?ml$/.test(path);
}

/** Only workflow definitions and prompt files are read or written through the editor. */
export function isEditablePath(path: string): boolean {
	return isWorkflowPath(path) || isPromptPath(path);
}

// --- reading ---------------------------------------------------------------------------------

export interface RepoWorkflows {
	defaultBranch: string;
	/** `.enzarb/workflows/*.yaml`, sorted. */
	workflows: string[];
	/** `.enzarb/prompts/*.md`, sorted (the paths the graph crate accepts). */
	prompts: string[];
}

export async function requireRead(api: RepoApi, login: string): Promise<void> {
	if ((await api.permission(login)) === 'none')
		throw new RepoError('You have no access to this repository.', 403);
}

export async function listRepoWorkflows(api: RepoApi): Promise<RepoWorkflows> {
	const defaultBranch = await api.defaultBranch();
	const files = async (dir: string, keep: (path: string) => boolean) =>
		(await api.listDir(dir.replace(/\/$/, ''), defaultBranch))
			.filter((e) => e.type === 'file')
			.map((e) => `${dir}${e.name}`)
			.filter(keep)
			.sort();
	const [workflows, prompts] = await Promise.all([
		files(WORKFLOW_DIR, isWorkflowPath),
		files(PROMPT_DIR, isPromptPath)
	]);
	return { defaultBranch, workflows, prompts };
}

// --- proposal token --------------------------------------------------------------------------

/** What a proposal token binds: who, where, and the commit the next step must build on. */
export interface ProposalClaims {
	project: string;
	repo: string;
	login: string;
	branch: string;
	base: string;
	/** Head of the default branch the proposal branch was created at. */
	baseSha: string;
	/** The commit this flow made, once step 2 ran. */
	headSha?: string;
	/** ms epoch. */
	exp: number;
}

const b64 = (b: Buffer | string) => Buffer.from(b).toString('base64url');
const mac = (secret: string, payload: string) =>
	createHmac('sha256', `enzarb-workflow-proposal\0${secret}`).update(payload).digest();

export function signProposal(secret: string, claims: ProposalClaims): string {
	const payload = b64(JSON.stringify(claims));
	return `${payload}.${b64(mac(secret, payload))}`;
}

/** The claims of a valid, unexpired token for the same project, repository and user. */
export function verifyProposal(
	secret: string,
	token: string,
	expect: { project: string; repo: string; login: string },
	now = Date.now()
): ProposalClaims {
	const bad = () => new RepoError('The proposal expired or is invalid. Start again.', 400);
	const [payload, sig, extra] = token.split('.');
	if (!payload || !sig || extra !== undefined) throw bad();
	const want = mac(secret, payload);
	const got = Buffer.from(sig, 'base64url');
	if (got.length !== want.length || !timingSafeEqual(got, want)) throw bad();
	let c: ProposalClaims;
	try {
		c = JSON.parse(Buffer.from(payload, 'base64url').toString('utf8')) as ProposalClaims;
	} catch {
		throw bad();
	}
	if (
		typeof c.exp !== 'number' ||
		c.exp < now ||
		c.project !== expect.project ||
		c.repo !== expect.repo ||
		c.login !== expect.login ||
		!isProposalBranch(c.branch)
	)
		throw bad();
	return c;
}

// --- proposing -------------------------------------------------------------------------------

const WRITE_ROLES = new Set(['admin', 'maintain', 'write']);

export async function requireWrite(api: RepoApi, login: string): Promise<void> {
	const role = await api.permission(login);
	if (!WRITE_ROLES.has(role))
		throw new RepoError('You need write access to this repository to propose a change.', 403);
}

/** Names that may appear in a branch, commit message and PR title: printable, one line. */
export { WORKFLOW_NAME_RE };

export function checkWorkflowName(name: string): void {
	if (!WORKFLOW_NAME_RE.test(name))
		throw new RepoError(
			'The workflow name must be letters, digits, spaces, ".", "_" or "-" (at most 64).',
			400
		);
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

function isProposalBranch(branch: unknown): boolean {
	return (
		typeof branch === 'string' && /^enzarb\/workflow-[a-z0-9-]{1,40}-[a-f0-9]{6}$/.test(branch)
	);
}

/** Paths, count and sizes; cheap, so it runs before any GitHub call. */
export function checkFiles(files: ChangedFile[]): void {
	if (files.length === 0) throw new RepoError('Nothing to commit.', 400);
	if (files.length > MAX_FILES) throw new RepoError(`At most ${MAX_FILES} files per change.`, 400);
	const seen = new Set<string>();
	let total = 0;
	for (const f of files) {
		if (!isEditablePath(f.path))
			throw new RepoError('Only workflow definitions and prompt files can be changed.', 400);
		if (seen.has(f.path)) throw new RepoError('A file is listed twice.', 400);
		seen.add(f.path);
		const size = Buffer.byteLength(f.text, 'utf8');
		if (size > MAX_FILE_BYTES) throw new RepoError('A file is too large.', 413);
		total += size;
	}
	if (total > MAX_TOTAL_BYTES) throw new RepoError('The change is too large.', 413);
}

export interface ProposalScope {
	project: string;
	repo: string;
	login: string;
	secret: string;
	now?: number;
}

/** Step 1: a new branch from the default branch head. Returns the proposal token. */
export async function createProposalBranch(
	api: RepoApi,
	scope: ProposalScope,
	workflowName: string
): Promise<{ branch: string; token: string }> {
	checkWorkflowName(workflowName);
	await requireWrite(api, scope.login);
	const base = await api.defaultBranch();
	const baseSha = await api.branchSha(base);
	if (!baseSha) throw new RepoError('The default branch has no head.');
	const branch = proposalBranch(workflowName);
	await api.createBranch(branch, baseSha);
	const claims: ProposalClaims = {
		project: scope.project,
		repo: scope.repo,
		login: scope.login,
		branch,
		base,
		baseSha,
		exp: (scope.now ?? Date.now()) + PROPOSAL_TTL_MS
	};
	return { branch, token: signProposal(scope.secret, claims) };
}

/**
 * Step 2: one commit with the changed files on the token's branch, only while the branch is
 * still where this flow left it. Returns the commit and the token for step 3.
 */
export async function commitProposal(
	api: RepoApi,
	scope: ProposalScope,
	token: string,
	workflowName: string,
	files: ChangedFile[]
): Promise<{ sha: string; token: string }> {
	checkFiles(files);
	checkWorkflowName(workflowName);
	const c = verifyProposal(scope.secret, token, scope, scope.now);
	await requireWrite(api, scope.login);
	if (c.branch === (await api.defaultBranch()))
		throw new RepoError('Refusing to commit to the default branch.', 400);
	const head = await api.branchSha(c.branch);
	if (!head || head !== (c.headSha ?? c.baseSha))
		throw new RepoError('The proposal branch changed. Start again.', 409);
	const sha = await api.commitFiles(
		c.branch,
		head,
		files,
		`chore(workflows): update ${workflowName} workflow\n\nProposed-by: @${scope.login} via enzarb`
	);
	return { sha, token: signProposal(scope.secret, { ...c, headSha: sha }) };
}

/** Step 3: the pull request, listing the files GitHub reports changed on the branch. */
export async function openProposal(
	api: RepoApi,
	scope: ProposalScope,
	token: string,
	workflowName: string
): Promise<{ number: number; url: string }> {
	checkWorkflowName(workflowName);
	const c = verifyProposal(scope.secret, token, scope, scope.now);
	if (!c.headSha) throw new RepoError('Nothing was committed yet.', 400);
	await requireWrite(api, scope.login);
	const paths = await api.changedPaths(c.baseSha, c.headSha);
	if (paths.length === 0 || !paths.every(isEditablePath))
		throw new RepoError('The proposal branch has unexpected changes.', 409);
	const list = paths.map((f) => `- \`${f}\``).join('\n');
	return api.openPullRequest({
		head: c.branch,
		base: c.base,
		title: `chore(workflows): update ${workflowName} workflow`,
		body: `Proposed by @${scope.login} with the enzarb workflow editor.\n\nChanged files:\n${list}\n`
	});
}

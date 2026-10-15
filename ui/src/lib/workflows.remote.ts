import { error } from '@sveltejs/kit';
import { command, query } from '$app/server';
import * as v from 'valibot';
import { normalizeRepoKey } from '#lib/projects.ts';
import { getConfig } from '#lib/server/config.ts';
import { createRepoTokenSource, type RepoPermissions } from '#lib/server/github-app.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import {
	checkFiles,
	commitProposal,
	createProposalBranch,
	githubRepoApi,
	isEditablePath,
	listRepoWorkflows,
	MAX_FILE_BYTES,
	MAX_FILES,
	openProposal,
	RepoError,
	requireRead,
	type ProposalScope,
	type RepoWorkflows
} from '#lib/server/workflow-repo.ts';

const READ: RepoPermissions = { contents: 'read', metadata: 'read' };
const WRITE: RepoPermissions = { contents: 'write', pull_requests: 'write', metadata: 'read' };

const RepoArg = v.pipe(v.string(), v.maxLength(200), v.regex(/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/));
const PathArg = v.pipe(v.string(), v.maxLength(300));
const WorkflowArg = v.pipe(v.string(), v.minLength(1), v.maxLength(64));
const TokenArg = v.pipe(v.string(), v.maxLength(2000));

let tokenSource: ReturnType<typeof createRepoTokenSource> | undefined;
const tokens = () => {
	if (!tokenSource) {
		const { githubAppId, githubAppPrivateKey } = getConfig();
		tokenSource = createRepoTokenSource({ appId: githubAppId, privateKey: githubAppPrivateKey });
	}
	return tokenSource;
};

/** The project's GitHub repositories as `owner/name`. */
function githubRepos(project: { repositories: { connector: string; url: string }[] }): string[] {
	return project.repositories
		.map((r) => normalizeRepoKey(r.connector, r.url))
		.filter((k): k is string => !!k && k.startsWith('github:'))
		.map((k) => k.slice('github:'.length));
}

/**
 * A repository API for `repo` of the caller's own project: 404 for a repository that is not in
 * the project. The token is the GitHub App's, down-scoped to the repository.
 */
async function apiFor(
	project: { repositories: { connector: string; url: string }[] },
	repo: string,
	perms: RepoPermissions
) {
	const key = repo.toLowerCase();
	if (!githubRepos(project).includes(key)) error(404, 'Repository not in this project');
	const token = await tokens().get(key, perms);
	if (!token) error(503, 'The enzarb GitHub App cannot access this repository');
	return { key, api: githubRepoApi(token, key) };
}

/** Maps a {@link RepoError} to its fixed message; GitHub's detail goes to the log only. */
async function guard<T>(f: () => Promise<T>): Promise<T> {
	try {
		return await f();
	} catch (e) {
		if (e instanceof RepoError) {
			if (e.detail) console.error('workflow editor: GitHub:', e.message, e.detail);
			const s = e.status;
			error(s && [400, 403, 409, 413].includes(s) ? s : 502, e.message);
		}
		throw e;
	}
}

export interface ProjectRepoWorkflows extends Partial<RepoWorkflows> {
	repo: string;
	error?: string;
}

/** Each repository of the project with its workflow definitions and prompt files (default branch). */
export const projectWorkflows = query(NameArg, async (name): Promise<ProjectRepoWorkflows[]> => {
	const { user, project } = await ownedProject(name);
	return Promise.all(
		githubRepos(project).map(async (repo) => {
			try {
				return await guard(async () => {
					const { api } = await apiFor(project, repo, READ);
					await requireRead(api, user.login);
					return { repo, ...(await listRepoWorkflows(api)) };
				});
			} catch (e) {
				const msg = (e as { body?: { message?: string } }).body?.message;
				return { repo, error: msg ?? 'Could not read the repository' };
			}
		})
	);
});

/** One editable file at the default branch head; `null` when it does not exist. */
export const workflowFile = query(
	v.object({ name: NameArg, repo: RepoArg, path: PathArg }),
	async ({ name, repo, path }) => {
		if (!isEditablePath(path)) error(400, 'Not a workflow definition or prompt file');
		const { user, project } = await ownedProject(name);
		const { api } = await apiFor(project, repo, READ);
		return guard(async () => {
			await requireRead(api, user.login);
			const f = await api.getFile(path, await api.defaultBranch());
			return f ? { path: f.path, text: f.text } : null;
		});
	}
);

async function writeScope(name: string, repo: string) {
	const { user, project } = await ownedProject(name);
	const { key, api } = await apiFor(project, repo, WRITE);
	const scope: ProposalScope = {
		project: name,
		repo: key,
		login: user.login,
		secret: getConfig().sessionJwtSecret
	};
	return { api, scope };
}

const ProposalArg = { name: NameArg, repo: RepoArg, workflow: WorkflowArg };

/** Step 1 of proposing a change: a new `enzarb/workflow-*` branch. Requires write access. */
export const startWorkflowProposal = command(v.object(ProposalArg), async (input) => {
	const { api, scope } = await writeScope(input.name, input.repo);
	return guard(() => createProposalBranch(api, scope, input.workflow));
});

const FileArg = v.object({
	path: PathArg,
	text: v.pipe(v.string(), v.maxLength(MAX_FILE_BYTES))
});

/** Step 2: one commit with the changed files on the proposal's branch. */
export const commitWorkflowProposal = command(
	v.object({
		...ProposalArg,
		token: TokenArg,
		files: v.pipe(v.array(FileArg), v.maxLength(MAX_FILES))
	}),
	async (input) => {
		// Sizes and paths before any GitHub call.
		await guard(async () => checkFiles(input.files));
		const { api, scope } = await writeScope(input.name, input.repo);
		return guard(() => commitProposal(api, scope, input.token, input.workflow, input.files));
	}
);

/** Step 3: the pull request into the default branch. */
export const openWorkflowProposal = command(
	v.object({ ...ProposalArg, token: TokenArg }),
	async (input) => {
		const { api, scope } = await writeScope(input.name, input.repo);
		return guard(() => openProposal(api, scope, input.token, input.workflow));
	}
);

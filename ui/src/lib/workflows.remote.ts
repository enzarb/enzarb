import { error } from '@sveltejs/kit';
import { command, query } from '$app/server';
import * as v from 'valibot';
import { normalizeRepoKey } from '#lib/projects.ts';
import { getConfig } from '#lib/server/config.ts';
import { repoInstallationToken, type RepoPermissions } from '#lib/server/github-app.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import {
	commitProposal,
	createProposalBranch,
	githubRepoApi,
	isEditablePath,
	listRepoWorkflows,
	MAX_FILE_BYTES,
	openProposal,
	RepoError,
	type RepoWorkflows
} from '#lib/server/workflow-repo.ts';

const READ: RepoPermissions = { contents: 'read', metadata: 'read' };
const WRITE: RepoPermissions = { contents: 'write', pull_requests: 'write', metadata: 'read' };

const RepoArg = v.pipe(v.string(), v.maxLength(200), v.regex(/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/));
const PathArg = v.pipe(v.string(), v.maxLength(300));
const WorkflowArg = v.pipe(v.string(), v.minLength(1), v.maxLength(100));
const BranchArg = v.pipe(v.string(), v.maxLength(100));

/** The project's GitHub repositories as `owner/name`. */
function githubRepos(project: { repositories: { connector: string; url: string }[] }): string[] {
	return project.repositories
		.map((r) => normalizeRepoKey(r.connector, r.url))
		.filter((k): k is string => !!k && k.startsWith('github:'))
		.map((k) => k.slice('github:'.length));
}

/**
 * A repository API for `repo` of the caller's own project `name`: 404 for a repository that is
 * not in the project. The token is the GitHub App's, minted per request and down-scoped.
 */
async function repoApi(name: string, repo: string, perms: RepoPermissions) {
	const { user, project } = await ownedProject(name);
	return { user, api: await apiFor(project, repo, perms) };
}

async function apiFor(
	project: { repositories: { connector: string; url: string }[] },
	repo: string,
	perms: RepoPermissions
) {
	const key = repo.toLowerCase();
	if (!githubRepos(project).includes(key)) error(404, 'Repository not in this project');
	const { githubAppId, githubAppPrivateKey } = getConfig();
	const token = await repoInstallationToken(key, perms, {
		appId: githubAppId,
		privateKey: githubAppPrivateKey
	});
	if (!token) error(503, 'The enzarb GitHub App cannot access this repository');
	return githubRepoApi(token, key);
}

async function guard<T>(f: () => Promise<T>): Promise<T> {
	try {
		return await f();
	} catch (e) {
		if (e instanceof RepoError) error(e.status === 403 ? 403 : 502, e.message);
		throw e;
	}
}

export interface ProjectRepoWorkflows extends Partial<RepoWorkflows> {
	repo: string;
	error?: string;
}

/** Each repository of the project with its workflow definitions and prompt files (default branch). */
export const projectWorkflows = query(NameArg, async (name): Promise<ProjectRepoWorkflows[]> => {
	const { project } = await ownedProject(name);
	return Promise.all(
		githubRepos(project).map(async (repo) => {
			try {
				return { repo, ...(await listRepoWorkflows(await apiFor(project, repo, READ))) };
			} catch (e) {
				const msg =
					e instanceof RepoError
						? e.message
						: ((e as { body?: { message?: string } }).body?.message ??
							'Could not read the repository');
				return { repo, error: msg };
			}
		})
	);
});

/** One editable file at the default branch head; `null` when it does not exist. */
export const workflowFile = query(
	v.object({ name: NameArg, repo: RepoArg, path: PathArg }),
	async ({ name, repo, path }) => {
		if (!isEditablePath(path)) error(400, 'Not a workflow definition or prompt file');
		const { api } = await repoApi(name, repo, READ);
		return guard(async () => {
			const f = await api.getFile(path, await api.defaultBranch());
			return f ? { path: f.path, text: f.text } : null;
		});
	}
);

const ProposalArg = { name: NameArg, repo: RepoArg, workflow: WorkflowArg };

/** Step 1 of proposing a change: a new `enzarb/workflow-*` branch. Requires write access. */
export const startWorkflowProposal = command(v.object(ProposalArg), async (input) => {
	const { user, api } = await repoApi(input.name, input.repo, WRITE);
	return guard(() => createProposalBranch(api, user.login, input.workflow));
});

const FileArg = v.object({
	path: PathArg,
	text: v.pipe(v.string(), v.maxLength(MAX_FILE_BYTES))
});

/** Step 2: one commit with the changed files on the proposal branch. */
export const commitWorkflowProposal = command(
	v.object({ ...ProposalArg, branch: BranchArg, files: v.pipe(v.array(FileArg), v.maxLength(20)) }),
	async (input) => {
		const { user, api } = await repoApi(input.name, input.repo, WRITE);
		return guard(() => commitProposal(api, user.login, input.branch, input.workflow, input.files));
	}
);

/** Step 3: the pull request into the default branch. */
export const openWorkflowProposal = command(
	v.object({
		...ProposalArg,
		branch: BranchArg,
		paths: v.pipe(v.array(PathArg), v.maxLength(20))
	}),
	async (input) => {
		const { user, api } = await repoApi(input.name, input.repo, WRITE);
		return guard(() => openProposal(api, user.login, input.branch, input.workflow, input.paths));
	}
);

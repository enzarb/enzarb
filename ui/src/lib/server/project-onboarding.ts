import type { Project, Repository } from '#lib/project-types.ts';
import {
	REPOSITORY_LIST_HELP,
	validateProjectName,
	type CreateProjectsOutcome,
	type PlanResult,
	type ProjectPlan,
	type Triggers
} from '#lib/onboarding.ts';
import { DISPLAY_NAME_MAX, buildProject, normalizeRepoKey } from '#lib/projects.ts';
import { KubeError, type ProjectStore } from './kube.ts';
import type { UserRepository, UserRepositoryList } from './user-repositories.ts';

/**
 * Live authorization for one repository key at the moment it is added to a Project: the
 * repository (URL and default branch come from GitHub) or null when the user may not add it.
 */
export type CheckRepository = (key: string) => Promise<UserRepository | null>;

/** Why the list offers nothing to select, or null when it offers something. */
export function listProblem(list: UserRepositoryList): { code: 'empty'; message: string } | null {
	if (list.repositories.length === 0)
		return {
			code: 'empty',
			message: `No repositories are available to you. ${REPOSITORY_LIST_HELP}`
		};
	return null;
}

/** Repository keys already held by Projects, mapped to the Project holding them. */
export function repoOwners(projects: Project[]): Map<string, Project> {
	const out = new Map<string, Project>();
	for (const p of projects) {
		for (const r of p.spec.repositories ?? []) {
			const key = normalizeRepoKey(r.connector, r.url);
			if (key) out.set(key, p);
		}
	}
	return out;
}

/** What the pickers offer: the live list plus which repos are taken (own Projects only). */
export function repositoryChoices(list: UserRepositoryList, projects: Project[], owner: string) {
	const taken: Record<string, string> = {};
	const projectNames = projects.filter((p) => p.spec.owner === owner).map((p) => p.metadata.name);
	for (const [key, p] of repoOwners(projects)) {
		if (p.spec.owner === owner) taken[key] = p.metadata.name;
	}
	return {
		repositories: list.repositories.map((r) => ({
			repo: r.repo,
			accountLogin: r.accountLogin,
			private: r.private,
			defaultBranch: r.defaultBranch
		})),
		taken,
		projectNames,
		fetchedAt: list.fetchedAt.toISOString(),
		problem: listProblem(list)
	};
}

export interface CreateProjectsDeps {
	store: ProjectStore;
	/** GitHub login of the requesting user; becomes `spec.owner`. */
	owner: string;
	/** Live access check for the requesting user. */
	check: CheckRepository;
}

function failed(plan: ProjectPlan, reason: string): PlanResult {
	return { name: plan.name, status: 'failed', repositories: plan.repositories, reason };
}

/**
 * Creates the planned Projects one by one and reports each outcome; a failure never stops the
 * others.
 *
 * Authorization: the browser only names repository keys. Each key is checked live with GitHub
 * (`deps.check`: the App can access it and the requester has write access now); the stored URL
 * and default branch come from that answer. The cluster is re-listed right before every create, so
 * a repository or name taken a moment ago (by this request or another) is refused.
 */
export async function createProjects(
	deps: CreateProjectsDeps,
	plans: ProjectPlan[],
	triggers: Triggers
): Promise<CreateProjectsOutcome> {
	const { store, owner, check } = deps;
	const results: PlanResult[] = [];
	for (const rawPlan of plans) {
		const plan = { ...rawPlan, name: rawPlan.name.trim() };
		results.push(await createOne(store, owner, check, plan, triggers));
	}
	return { ok: true, results };
}

/**
 * Serializes "list Projects, then create" within this process, so concurrent requests on one UI
 * replica cannot both claim the same repository. Across replicas this is only best effort:
 * cluster-wide enforcement is tracked in enzarb/enzarb-sandbox-operator ("Enforce that a
 * repository belongs to at most one Project").
 */
let lock: Promise<unknown> = Promise.resolve();
function exclusive<T>(fn: () => Promise<T>): Promise<T> {
	const run = lock.then(fn, fn);
	lock = run.catch(() => {});
	return run;
}

async function createOne(
	store: ProjectStore,
	owner: string,
	check: CheckRepository,
	plan: ProjectPlan,
	triggers: Triggers
): Promise<PlanResult> {
	const nameProblem = validateProjectName(plan.name);
	if (nameProblem) return failed(plan, nameProblem);
	if (plan.repositories.length === 0) return failed(plan, 'Select at least one repository');
	if (new Set(plan.repositories).size !== plan.repositories.length)
		return failed(plan, 'A repository is listed more than once');

	const repositories: Repository[] = [];
	for (const key of plan.repositories) {
		const row = await check(key);
		if (!row)
			return failed(plan, `${key.replace(/^github:/, '').slice(0, 100)} is not available to you`);
		repositories.push({
			url: `https://github.com/${row.repo.replace(/^github:/, '')}`,
			connector: 'github',
			defaultBranch: row.defaultBranch
		});
	}

	return exclusive(() => claimAndCreate(store, owner, plan, repositories, triggers));
}

async function claimAndCreate(
	store: ProjectStore,
	owner: string,
	plan: ProjectPlan,
	repositories: Repository[],
	triggers: Triggers
): Promise<PlanResult> {
	try {
		const taken = repoOwners(await store.list());
		for (const key of plan.repositories) {
			const holder = taken.get(key);
			if (holder) {
				const where = holder.spec.owner === owner ? `"${holder.metadata.name}"` : 'another project';
				return failed(plan, `${key.replace(/^github:/, '')} is already in ${where}`);
			}
		}
		const project = buildProject(
			{
				name: plan.name,
				displayName: plan.displayName?.trim() ?? '',
				repositories,
				triggers,
				maxRuns: plan.maxRuns
			},
			owner
		);
		await store.create(project);
		return { name: plan.name, status: 'created', repositories: plan.repositories };
	} catch (e) {
		if (e instanceof KubeError && e.kind === 'conflict')
			return failed(plan, `A project named "${plan.name}" already exists`);
		console.error(`could not create project ${plan.name}:`, (e as Error).message);
		return failed(plan, 'The cluster refused or could not create the project');
	}
}

export type SetDisplayNameOutcome =
	{ ok: true; displayName: string } | { ok: false; code: 'not-found' | 'invalid'; message: string };

/** Renames a Project (`spec.displayName`). Owner only; no restart, not part of the pod. */
export async function setDisplayName(
	deps: { store: ProjectStore; owner: string },
	name: string,
	input: string
): Promise<SetDisplayNameOutcome> {
	const displayName = input.trim();
	if (!displayName) return { ok: false, code: 'invalid', message: 'Enter a display name' };
	if (displayName.length > DISPLAY_NAME_MAX)
		return {
			ok: false,
			code: 'invalid',
			message: `Display name must be at most ${DISPLAY_NAME_MAX} characters`
		};
	return exclusive(async () => {
		const project = await deps.store.get(name);
		if (!project || project.spec.owner !== deps.owner)
			return { ok: false, code: 'not-found', message: 'Project not found' };
		await deps.store.update(name, { displayName });
		return { ok: true, displayName };
	});
}

export interface SetRepositoriesDeps {
	store: ProjectStore;
	owner: string;
	/** Live access check for the requesting user (added repositories only). */
	check: CheckRepository;
	/** Cancels the open work items of repositories that left the Project (best effort). */
	work?: {
		cancelForRemovedRepos(project: string, removedKeys: string[]): Promise<unknown>;
	};
}

export type SetRepositoriesOutcome =
	| { ok: true; repositories: string[] }
	| {
			ok: false;
			code: 'not-found' | 'invalid';
			message: string;
	  };

/**
 * Replaces a Project's `spec.repositories` with the given repository keys. Owner only.
 *
 * Authorization matches `createProjects`: the browser only names keys. A key already in the
 * Project keeps its stored entry (so archived repositories, no longer listed, can still be kept
 * or removed); every added key is checked live with GitHub (`deps.check`), whose answer gives the
 * URL and default branch. A key held by any other Project is refused.
 * No restart: repositories are not part of the sandbox pod (see contracts.md).
 */
export async function setRepositories(
	deps: SetRepositoriesDeps,
	name: string,
	keys: string[]
): Promise<SetRepositoriesOutcome> {
	const { store, owner, check } = deps;
	const wanted = keys.map((k) => k.trim().toLowerCase());
	if (wanted.length === 0)
		return { ok: false, code: 'invalid', message: 'Select at least one repository' };
	if (new Set(wanted).size !== wanted.length)
		return { ok: false, code: 'invalid', message: 'A repository is listed more than once' };

	return exclusive(async () => {
		const project = await store.get(name);
		if (!project || project.spec.owner !== owner)
			return { ok: false, code: 'not-found', message: 'Project not found' };

		const current = new Map<string, Repository>();
		for (const r of project.spec.repositories ?? []) {
			const key = normalizeRepoKey(r.connector, r.url);
			if (key) current.set(key, r);
		}
		const taken = repoOwners(await store.list());
		const repositories: Repository[] = [];
		for (const key of wanted) {
			const kept = current.get(key);
			if (kept) {
				repositories.push(kept);
				continue;
			}
			const shown = key.replace(/^github:/, '').slice(0, 100);
			const holder = taken.get(key);
			if (holder && holder.metadata.name !== name) {
				const where = holder.spec.owner === owner ? `"${holder.metadata.name}"` : 'another project';
				return { ok: false, code: 'invalid', message: `${shown} is already in ${where}` };
			}
			const row = await check(key);
			if (!row) return { ok: false, code: 'invalid', message: `${shown} is not available to you` };
			repositories.push({
				url: `https://github.com/${row.repo.replace(/^github:/, '')}`,
				connector: 'github',
				defaultBranch: row.defaultBranch
			});
		}
		await store.update(name, { repositories });
		const removed = [...current.keys()].filter((k) => !wanted.includes(k));
		if (removed.length > 0 && deps.work) {
			// The update stands even if this fails: the owner can still cancel the items from the UI
			// and closing the issue cancels them too.
			try {
				await deps.work.cancelForRemovedRepos(name, removed);
			} catch (e) {
				console.warn(
					`could not cancel work items of removed repositories in ${name}:`,
					(e as Error).message
				);
			}
		}
		return { ok: true, repositories: wanted };
	});
}

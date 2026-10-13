import type { Project, Repository } from '#lib/project-types.ts';
import {
	SNAPSHOT_MAX_AGE_MS,
	validateProjectName,
	type CreateProjectsOutcome,
	type PlanResult,
	type ProjectPlan,
	type Triggers
} from '#lib/onboarding.ts';
import { buildProject, normalizeRepoKey } from '#lib/projects.ts';
import { KubeError, type ProjectStore } from './kube.ts';
import type { UserRepository, UserRepositorySnapshot } from './user-repositories.ts';

/** Why a snapshot cannot be used for selecting repositories, or null when it can. */
export function snapshotProblem(
	snapshot: UserRepositorySnapshot,
	now: Date
): { code: 'no-snapshot' | 'stale-snapshot'; message: string } | null {
	if (!snapshot.fetchedAt || snapshot.repositories.length === 0) {
		return {
			code: 'no-snapshot',
			message:
				'No repositories are available to you yet. Install the GitHub App and refresh the repository list.'
		};
	}
	if (now.getTime() - snapshot.fetchedAt.getTime() > SNAPSHOT_MAX_AGE_MS) {
		return {
			code: 'stale-snapshot',
			message:
				'Your repository list is more than 24 hours old. Refresh it from GitHub, then try again.'
		};
	}
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

/** What the new-project page offers: the fresh snapshot plus which repos are taken (own Projects only). */
export function repositoryChoices(
	snapshot: UserRepositorySnapshot,
	projects: Project[],
	owner: string,
	now = new Date()
) {
	const taken: Record<string, string> = {};
	const projectNames = projects.filter((p) => p.spec.owner === owner).map((p) => p.metadata.name);
	for (const [key, p] of repoOwners(projects)) {
		if (p.spec.owner === owner) taken[key] = p.metadata.name;
	}
	return {
		repositories: snapshot.repositories.map((r) => ({
			repo: r.repo,
			accountLogin: r.accountLogin,
			private: r.private,
			defaultBranch: r.defaultBranch
		})),
		taken,
		projectNames,
		problem: snapshotProblem(snapshot, now)
	};
}

export interface CreateProjectsDeps {
	store: ProjectStore;
	/** GitHub login of the requesting user; becomes `spec.owner`. */
	owner: string;
	/** The requesting user's own snapshot, read by the caller. */
	snapshot: UserRepositorySnapshot;
	now?: Date;
}

function failed(plan: ProjectPlan, reason: string): PlanResult {
	return { name: plan.name, status: 'failed', repositories: plan.repositories, reason };
}

/**
 * Creates the planned Projects one by one and reports each outcome; a failure never stops the
 * others.
 *
 * Authorization: the browser only names repository keys. Each key must match a row of the
 * requester's snapshot, which must be younger than 24 hours; the stored URL and default branch
 * are built here from that row. The cluster is re-listed right before every create, so a
 * repository or name taken a moment ago (by this request or another) is refused.
 */
export async function createProjects(
	deps: CreateProjectsDeps,
	plans: ProjectPlan[],
	triggers: Triggers
): Promise<CreateProjectsOutcome> {
	const { store, owner, snapshot } = deps;
	const problem = snapshotProblem(snapshot, deps.now ?? new Date());
	if (problem) return { ok: false, ...problem };

	const known = new Map<string, UserRepository>(snapshot.repositories.map((r) => [r.repo, r]));
	const results: PlanResult[] = [];
	for (const rawPlan of plans) {
		const plan = { ...rawPlan, name: rawPlan.name.trim() };
		results.push(await createOne(store, owner, known, plan, triggers));
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
	known: Map<string, UserRepository>,
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
		const row = known.get(key);
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

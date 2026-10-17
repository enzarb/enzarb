import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { MAX_PROJECTS_PER_REQUEST } from '#lib/onboarding.ts';
import { CREATE_REPO_PURPOSE, setCreateRepoIntentCookie } from '#lib/server/auth.ts';
import { getConfig } from '#lib/server/config.ts';
import { buildIntent, signIntent } from '#lib/server/create-repo-intent.ts';
import { DISPLAY_NAME_MAX, MaxRunsSchema } from '#lib/projects.ts';
import { SandboxResourcesSchema } from '#lib/sandbox-resources.ts';
import { KubeError, getProjectStore } from '#lib/server/kube.ts';
import { projectCache } from '#lib/server/project-cache.ts';
import {
	createProjects as createProjectsFor,
	setDisplayName as setDisplayNameFor,
	setRepositories as setRepositoriesFor,
	repositoryChoices as repositoryChoicesFor
} from '#lib/server/project-onboarding.ts';
import { getRepositoryAccess } from '#lib/server/repo-access.ts';
import type { UserRepositoryList } from '#lib/server/user-repositories.ts';
import { projectService } from '#lib/server/project-service.ts';

function service() {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	return projectService(getProjectStore(), user.login);
}

const NameArg = v.pipe(v.string(), v.maxLength(63));

export const listProjects = query(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const projects = await service().list();
	projectCache.put(user.login, projects); // listing anyway: refresh the shared cache
	return projects;
});

export const getProject = query(NameArg, async (name) => {
	const project = await service().get(name);
	if (!project) error(404, 'Project not found');
	return project;
});

/** The live list for display; a GitHub failure becomes an empty list with a problem. */
async function liveList(login: string, refresh = false) {
	try {
		return { list: await getRepositoryAccess().list(login, { refresh }), failed: false };
	} catch (e) {
		console.error('could not list repositories from GitHub:', (e as Error).message);
		const list: UserRepositoryList = { repositories: [], fetchedAt: new Date() };
		return { list, failed: true };
	}
}

/**
 * Repositories the user may pick (live from GitHub through the App, cached about a minute for
 * display) and which of them are already in a Project.
 */
export const repositoryChoices = query(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const { list, failed } = await liveList(user.login);
	const choices = repositoryChoicesFor(list, await getProjectStore().list(), user.login);
	if (failed)
		return {
			...choices,
			problem: {
				code: 'unavailable',
				message: 'Could not read your repositories from GitHub. Use Refresh to try again.'
			}
		};
	return choices;
});

/** The picker's Refresh: asks GitHub again, bypassing the display cache. */
export const refreshRepositoryChoices = command(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	await liveList(user.login, true);
	await repositoryChoices().refresh();
});

/** Live authorization for the signed-in user, used when a repository is added to a Project. */
const liveCheck = (login: string) => (key: string) => getRepositoryAccess().check(login, key);

const PlanSchema = v.object({
	name: v.pipe(v.string(), v.maxLength(200)),
	displayName: v.optional(v.pipe(v.string(), v.maxLength(DISPLAY_NAME_MAX))),
	repositories: v.pipe(v.array(v.pipe(v.string(), v.maxLength(300))), v.maxLength(500)),
	maxRuns: v.optional(MaxRunsSchema)
});

const CreateProjectsSchema = v.object({
	plans: v.pipe(v.array(PlanSchema), v.minLength(1), v.maxLength(MAX_PROJECTS_PER_REQUEST)),
	triggers: v.object({ issues: v.boolean(), changeRequests: v.boolean(), mentions: v.boolean() })
});

/**
 * Creates one or many Projects. Repositories are accepted only as keys, each checked live with
 * GitHub for the caller (see `createProjects`); URLs are never taken from the browser.
 */
export const createProjects = command(CreateProjectsSchema, async (input) => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	if (!getRepositoryAccess().configured)
		return {
			ok: false as const,
			code: 'unavailable' as const,
			message: 'The GitHub App is not configured, so repositories cannot be checked.'
		};
	const outcome = await createProjectsFor(
		{ store: getProjectStore(), owner: user.login, check: liveCheck(user.login) },
		input.plans,
		input.triggers
	);
	projectCache.invalidate(user.login);
	return outcome;
});

const StartCreateRepoSchema = v.object({
	owner: v.pipe(v.string(), v.maxLength(100)),
	name: v.pipe(v.string(), v.maxLength(200)),
	visibility: v.picklist(['private', 'public']),
	description: v.pipe(v.string(), v.maxLength(1000))
});

/** The form's page; the only place the re-authorization returns to. */
const CREATE_REPO_RETURN = '/projects/new';

/**
 * First half of "create a new GitHub repository". A remote function never holds a user token
 * (it only exists inside the OAuth callback), so this validates the request, stores it as a
 * signed single-use intent cookie and sends the browser through `/login/github`, whose callback
 * does the one creation call. The owner must be one of the caller's own installations.
 */
export const startCreateRepo = command(StartCreateRepoSchema, async (input) => {
	const { locals, cookies } = getRequestEvent();
	const { user } = locals;
	if (!user) error(401, 'Not signed in');
	const built = buildIntent(
		{
			user,
			list: (await liveList(user.login)).list,
			projects: await getProjectStore().list(),
			returnPath: CREATE_REPO_RETURN
		},
		input
	);
	if (!built.ok) return { ok: false as const, message: built.message };
	setCreateRepoIntentCookie(cookies, await signIntent(getConfig().sessionJwtSecret, built.intent));
	const href = `/login/github?purpose=${CREATE_REPO_PURPOSE}&return=${encodeURIComponent(CREATE_REPO_RETURN)}`;
	return { ok: true as const, href };
});

export const setMaxRuns = command(
	v.object({ name: NameArg, maxRuns: v.nullable(MaxRunsSchema) }),
	async ({ name, maxRuns }) => {
		try {
			await service().setMaxRuns(name, maxRuns);
			projectCache.invalidate(getRequestEvent().locals.user!.login);
		} catch (e) {
			if (e instanceof KubeError && e.kind === 'not-found') error(404, 'Project not found');
			throw e;
		}
		await getProject(name).refresh();
		await listProjects().refresh();
	}
);

/** Owner-only; the change is queued by the operator until running jobs finish (drain-first). */
export const setSandboxResources = command(
	v.object({ name: NameArg, ...SandboxResourcesSchema.entries }),
	async ({ name, ...input }) => {
		try {
			await service().setSandboxResources(name, input);
			projectCache.invalidate(getRequestEvent().locals.user!.login);
		} catch (e) {
			if (e instanceof KubeError && e.kind === 'not-found') error(404, 'Project not found');
			if (e instanceof KubeError && e.kind === 'invalid') error(400, e.message);
			throw e;
		}
		await getProject(name).refresh();
		await listProjects().refresh();
	}
);

/**
 * Owner-only: replaces the Project's repositories. Keys only; added ones are checked live with
 * GitHub (see `setRepositories`). Applies without a sandbox restart or drain.
 */
export const setRepositories = command(
	v.object({
		name: NameArg,
		repositories: v.pipe(
			v.array(v.pipe(v.string(), v.maxLength(300))),
			v.minLength(1),
			v.maxLength(500)
		)
	}),
	async ({ name, repositories }) => {
		const { user } = getRequestEvent().locals;
		if (!user) error(401, 'Not signed in');
		const outcome = await setRepositoriesFor(
			{
				store: getProjectStore(),
				owner: user.login,
				// Tasks of removed repositories are the engine's to end (ADR-0022); the UI decides nothing.
				check: liveCheck(user.login)
			},
			name,
			repositories
		);
		if (!outcome.ok && outcome.code === 'not-found') error(404, 'Project not found');
		if (outcome.ok) {
			projectCache.invalidate(user.login);
			await getProject(name).refresh();
			await listProjects().refresh();
		}
		return outcome;
	}
);

/** Owner-only: renames the Project's `spec.displayName` (`metadata.name` is immutable). */
export const setDisplayName = command(
	v.object({
		name: NameArg,
		displayName: v.pipe(v.string(), v.trim(), v.minLength(1), v.maxLength(DISPLAY_NAME_MAX))
	}),
	async ({ name, displayName }) => {
		const { user } = getRequestEvent().locals;
		if (!user) error(401, 'Not signed in');
		const outcome = await setDisplayNameFor(
			{ store: getProjectStore(), owner: user.login },
			name,
			displayName
		);
		if (!outcome.ok && outcome.code === 'not-found') error(404, 'Project not found');
		if (outcome.ok) {
			projectCache.invalidate(user.login);
			await getProject(name).refresh();
			await listProjects().refresh();
		}
		return outcome;
	}
);

export const deleteProject = command(NameArg, async (name) => {
	try {
		await service().delete(name);
		projectCache.invalidate(getRequestEvent().locals.user!.login);
	} catch (e) {
		if (e instanceof KubeError && e.kind === 'not-found') error(404, 'Project not found');
		throw e;
	}
	await listProjects().refresh();
});

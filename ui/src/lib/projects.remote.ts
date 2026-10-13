import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { MAX_PROJECTS_PER_REQUEST } from '#lib/onboarding.ts';
import { CREATE_REPO_PURPOSE, setCreateRepoIntentCookie } from '#lib/server/auth.ts';
import { getConfig } from '#lib/server/config.ts';
import { buildIntent, signIntent } from '#lib/server/create-repo-intent.ts';
import { MaxRunsSchema } from '#lib/projects.ts';
import { SandboxResourcesSchema } from '#lib/sandbox-resources.ts';
import { getSql } from '#lib/server/db.ts';
import { KubeError, getProjectStore } from '#lib/server/kube.ts';
import { projectCache } from '#lib/server/project-cache.ts';
import {
	createProjects as createProjectsFor,
	repositoryChoices as repositoryChoicesFor
} from '#lib/server/project-onboarding.ts';
import { getUserRepositorySnapshot } from '#lib/server/user-repositories.ts';
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

/** Repositories the user may pick (their snapshot) and which of them are already in a Project. */
export const repositoryChoices = query(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const snapshot = await getUserRepositorySnapshot(getSql(), user.id);
	return repositoryChoicesFor(snapshot, await getProjectStore().list(), user.login);
});

const PlanSchema = v.object({
	name: v.pipe(v.string(), v.maxLength(200)),
	displayName: v.optional(v.pipe(v.string(), v.maxLength(100))),
	repositories: v.pipe(v.array(v.pipe(v.string(), v.maxLength(300))), v.maxLength(500)),
	maxRuns: v.optional(MaxRunsSchema)
});

const CreateProjectsSchema = v.object({
	plans: v.pipe(v.array(PlanSchema), v.minLength(1), v.maxLength(MAX_PROJECTS_PER_REQUEST)),
	triggers: v.object({ issues: v.boolean(), changeRequests: v.boolean(), mentions: v.boolean() })
});

/**
 * Creates one or many Projects. Repositories are accepted only as keys that match the caller's
 * fresh repository snapshot (see `createProjects`); URLs are never taken from the browser.
 */
export const createProjects = command(CreateProjectsSchema, async (input) => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const snapshot = await getUserRepositorySnapshot(getSql(), user.id);
	const outcome = await createProjectsFor(
		{ store: getProjectStore(), owner: user.login, snapshot },
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
			snapshot: await getUserRepositorySnapshot(getSql(), user.id),
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

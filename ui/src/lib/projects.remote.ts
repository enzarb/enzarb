import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { MAX_PROJECTS_PER_REQUEST } from '#lib/onboarding.ts';
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
	repositories: v.pipe(v.array(v.pipe(v.string(), v.maxLength(300))), v.maxLength(500))
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

import { error, invalid, redirect } from '@sveltejs/kit';
import { command, form, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { CreateProjectSchema } from '#lib/projects.ts';
import { getConfig } from '#lib/server/config.ts';
import { KubeError, getProjectStore } from '#lib/server/kube.ts';
import { projectCache } from '#lib/server/project-cache.ts';
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

/** RuntimeClass names the Docker select offers (ENZARB_SANDBOX_RUNTIME_CLASSES). */
export const runtimeClasses = query(async () => getConfig().sandboxRuntimeClasses);

export const createProject = form(CreateProjectSchema, async (input, issue) => {
	if (input.docker && !getConfig().sandboxRuntimeClasses.includes(input.runtimeClassName)) {
		invalid(
			issue.runtimeClassName(
				`Runtime class must be one of: ${getConfig().sandboxRuntimeClasses.join(', ')}`
			)
		);
	}
	try {
		await service().create(input);
		projectCache.invalidate(getRequestEvent().locals.user!.login);
	} catch (e) {
		if (e instanceof KubeError && e.kind === 'conflict') {
			invalid(issue.name(`A project named "${input.name}" already exists`));
		}
		throw e;
	}
	redirect(303, `/projects/${input.name}`);
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

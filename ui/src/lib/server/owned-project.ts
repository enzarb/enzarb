import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import * as v from 'valibot';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';

export const NameArg = v.pipe(v.string(), v.maxLength(63));

/** The signed-in owner of `name`; 404 for missing and foreign projects. */
export async function ownedProject(name: string) {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const project = await projectService(getProjectStore(), user.login).get(name);
	if (!project) error(404, 'Project not found');
	return { user, project };
}

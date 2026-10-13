import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import * as v from 'valibot';
import { getSql } from '#lib/server/db.ts';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectCache } from '#lib/server/project-cache.ts';
import { issueRealtimeConnection } from '#lib/server/realtime-connection.ts';
import { WorkError, workService } from '#lib/server/work-service.ts';
import { SubmitActionSchema } from '#lib/work-items.ts';

async function service() {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	const projects = await projectCache.list(getProjectStore(), user.login);
	return {
		user,
		work: workService(
			getSql(),
			projects.map((p) => p.name)
		)
	};
}

/** Sets the HttpOnly realtime cookie for the session user; returns the stream URL, never a token. */
export const realtimeConnection = command(async () => issueRealtimeConnection(getRequestEvent()));

export const listWorkItems = query(
	v.optional(v.pipe(v.string(), v.maxLength(63))),
	async (project) => (await service()).work.list(project)
);

export const listNeedsYou = query(async () => (await service()).work.listNeedsYou());

export const getWorkItemPlan = query(v.pipe(v.number(), v.integer(), v.minValue(1)), async (id) =>
	(await service()).work.plan(id)
);

export const getWorkItem = query(v.pipe(v.number(), v.integer(), v.minValue(1)), async (id) => {
	const item = await (await service()).work.get(id);
	if (!item) error(404, 'Work item not found');
	return item;
});

export const submitAction = command(SubmitActionSchema, async ({ id, action }) => {
	const { user, work } = await service();
	try {
		await work.submitAction(id, { login: user.login, id: user.githubId }, action);
	} catch (e) {
		if (e instanceof WorkError) error(e.kind === 'not-found' ? 404 : 409, e.message);
		throw e;
	}
});

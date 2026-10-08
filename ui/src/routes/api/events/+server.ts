import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { getProjectStore } from '#lib/server/kube.ts';
import { projectService } from '#lib/server/project-service.ts';
import { getWorkEventHub } from '#lib/server/work-hub.ts';
import { sseStream } from '#lib/server/sse.ts';

const OWNED_TTL_MS = 30_000;

/**
 * Server-sent events: `data: {"id": <work item id>}` for every change to a work item of a
 * Project the user owns. Clients refresh the queries for that id.
 */
export const GET: RequestHandler = ({ locals, request }) => {
	const user = locals.user;
	if (!user) error(401, 'Not signed in');

	const projects = projectService(getProjectStore(), user.login);
	let owned = new Set<string>();
	let loadedAt = 0;
	const ownedNow = async () => {
		if (Date.now() - loadedAt > OWNED_TTL_MS) {
			owned = new Set((await projects.list()).map((p) => p.name));
			loadedAt = Date.now();
		}
		return owned;
	};

	return sseStream(request.signal, (send) =>
		getWorkEventHub().subscribe(async ({ id, project }) => {
			try {
				if (project && (await ownedNow()).has(project)) send({ id });
			} catch {
				/* a failed ownership lookup drops this one notification */
			}
		})
	);
};

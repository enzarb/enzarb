import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { getRepoContents } from '$lib/gitea';

function resolveNamespace() {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.slug === params.namespace);
	if (!org) error(403, 'Forbidden');
	return org;
}

export const getGitTree = query(
	z.object({ path: z.string().default(''), ref: z.string().default('main') }),
	async ({ path, ref }) => {
		const { params } = getRequestEvent();
		const org = resolveNamespace();
		return getRepoContents(org.id, params.project!, path, ref);
	}
);

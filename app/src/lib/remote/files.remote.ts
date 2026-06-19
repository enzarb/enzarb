import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { getRepoContents } from '$lib/gitea';

function requireOrgMember(orgId: string) {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	if (!locals.session.orgs.find((o) => o.id === orgId)) error(403, 'Forbidden');
}

export const getGitTree = query(
	z.object({ path: z.string().default(''), ref: z.string().default('main') }),
	async ({ path, ref }) => {
		const { params } = getRequestEvent();
		requireOrgMember(params.org!);
		return getRepoContents(params.org!, params.project!, path, ref);
	}
);

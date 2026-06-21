import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { z } from 'zod/v4';
import { getRepoContents } from '$lib/gitea';
import { resolveOrg } from './guard';

export const getGitTree = query(
	z.object({ path: z.string().default(''), ref: z.string().default('main') }),
	async ({ path, ref }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return getRepoContents(org.id, params.project!, path, ref);
	}
);

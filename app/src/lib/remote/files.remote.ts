import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { z } from 'zod/v4';
import { getRepoContents, listBranches, listTags } from '$lib/gitea';
import { resolveOrg } from './guard';

export const getGitContents = query(
	z.object({ path: z.string().default(''), ref: z.string().default('main') }),
	async ({ path, ref }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return getRepoContents(org.slug, params.project!, path, ref);
	}
);

export const getGitRefs = query(async () => {
	const { params } = getRequestEvent();
	const org = resolveOrg();
	const [branches, tags] = await Promise.all([
		listBranches(org.slug, params.project!),
		listTags(org.slug, params.project!)
	]);
	return {
		branches: (branches as any[] ?? []).map((b: any) => b.name as string),
		tags: (tags as any[] ?? []).map((t: any) => t.name as string)
	};
});

import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { z } from 'zod/v4';
import { getRepoContents, listBranches, listTags, listCommits, getCommit, getBlame } from '$lib/gitea';
import { resolveOrg } from './guard';

export const getGitContents = query(
	z.object({ path: z.string().default(''), ref: z.string().default('main') }),
	async ({ path, ref }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return getRepoContents(org.slug, params.project!, path, ref);
	}
);

export const getGitCommits = query(
	z.object({ ref: z.string().default('main'), path: z.string().optional(), page: z.number().default(1) }),
	async ({ ref, path, page }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return listCommits(org.slug, params.project!, ref, path, page);
	}
);

export const getGitCommit = query(
	z.object({ sha: z.string() }),
	async ({ sha }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return getCommit(org.slug, params.project!, sha);
	}
);

export const getGitBlame = query(
	z.object({ filepath: z.string(), ref: z.string().default('main') }),
	async ({ filepath, ref }) => {
		const { params } = getRequestEvent();
		const org = resolveOrg();
		return getBlame(org.slug, params.project!, filepath, ref);
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

import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listRepositories, listTags, getManifest, deleteManifest } from '$lib/zot';

function requireOrgMember(orgId: string, minRole?: 'admin') {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.id === orgId);
	if (!org) error(403, 'Forbidden');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

export const getRepositories = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	const all = await listRepositories();
	return all.filter((r) => r.name.startsWith(`${params.org!}/`));
});

export const getRepoTags = query(z.string(), async (repo) => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	if (!repo.startsWith(`${params.org!}/`)) error(403, 'Forbidden');
	return listTags(repo);
});

export const getImageManifest = query(
	z.object({ repo: z.string(), reference: z.string() }),
	async ({ repo, reference }) => {
		const { params } = getRequestEvent();
		requireOrgMember(params.org!);
		if (!repo.startsWith(`${params.org!}/`)) error(403, 'Forbidden');
		return getManifest(repo, reference);
	}
);

export const removeImage = command(
	z.object({ repo: z.string(), digest: z.string() }),
	async ({ repo, digest }) => {
		const { params } = getRequestEvent();
		requireOrgMember(params.org!, 'admin');
		if (!repo.startsWith(`${params.org!}/`)) error(403, 'Forbidden');
		await deleteManifest(repo, digest);
	}
);

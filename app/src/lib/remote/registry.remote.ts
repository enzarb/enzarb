import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listRepositories, listTags, getManifest, deleteManifest } from '$lib/zot';

function resolveNamespace(minRole?: 'admin') {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.slug === params.namespace);
	if (!org) error(403, 'Forbidden');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

export const getRepositories = query(async () => {
	const org = resolveNamespace();
	const all = await listRepositories();
	return all.filter((r) => r.name.startsWith(`${org.slug}/`));
});

export const getRepoTags = query(z.string(), async (repo) => {
	const org = resolveNamespace();
	if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
	return listTags(repo);
});

export const getImageManifest = query(
	z.object({ repo: z.string(), reference: z.string() }),
	async ({ repo, reference }) => {
		const org = resolveNamespace();
		if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
		return getManifest(repo, reference);
	}
);

export const removeImage = command(
	z.object({ repo: z.string(), digest: z.string() }),
	async ({ repo, digest }) => {
		const org = resolveNamespace('admin');
		if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
		await deleteManifest(repo, digest);
	}
);

import { query, command } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listRepositories, listTags, getManifest, deleteManifest } from '$lib/zot';
import { resolveOrg, requirePrivilege } from './guard';

export const getRepositories = query(async () => {
	const org = resolveOrg();
	const all = await listRepositories();
	return all.filter((r) => r.name.startsWith(`${org.slug}/`));
});

export const getRepoTags = query(z.string(), async (repo) => {
	const org = resolveOrg();
	if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
	return listTags(repo);
});

export const getImageManifest = query(
	z.object({ repo: z.string(), reference: z.string() }),
	async ({ repo, reference }) => {
		const org = resolveOrg();
		if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
		return getManifest(repo, reference);
	}
);

export const removeImage = command(
	z.object({ repo: z.string(), digest: z.string() }),
	async ({ repo, digest }) => {
		const org = requirePrivilege('registry.delete');
		if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
		await deleteManifest(repo, digest);
	}
);

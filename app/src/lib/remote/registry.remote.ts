import { query, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listRepositories, listTags, getManifest, getBlob, deleteManifest } from '$lib/zot';
import { resolveOrg, requirePrivilege } from './guard';

export const getRepositories = query(async () => {
	const org = resolveOrg();
	const { params } = getRequestEvent();
	const all = await listRepositories();
	if (params.project) {
		const prefix = `${org.slug}/${params.project}/`;
		const exact = `${org.slug}/${params.project}`;
		return all.filter((r) => r.name.startsWith(prefix) || r.name === exact);
	}
	return all.filter((r) => r.name.startsWith(`${org.slug}/`));
});

export const getProjectRepoDetails = query(z.string().optional(), async () => {
	const org = resolveOrg();
	const { params } = getRequestEvent();
	const projectPrefix = `${org.slug}/${params.project}/`;
	const exactName = `${org.slug}/${params.project}`;
	const all = await listRepositories();
	const projectRepos = all.filter((r) => r.name.startsWith(projectPrefix) || r.name === exactName);
	return Promise.all(
		projectRepos.map(async (repo) => {
			const tagList = await listTags(repo.name);
			return { name: repo.name, tagCount: (tagList.tags ?? []).length };
		})
	);
});

export const getRepoTags = query(z.string(), async (repo) => {
	const org = resolveOrg();
	if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
	return listTags(repo);
});

type ResolvedLayers = {
	layers: { digest: string; size: number }[];
	totalSize: number;
	configDigest?: string;
};

// Resolves a tag/digest reference down to its concrete layer+config blobs,
// following one level into a manifest index (multi-arch) by picking the
// linux/amd64 entry (or the first entry if none matches).
async function resolveLayers(repo: string, reference: string): Promise<ResolvedLayers | null> {
	const manifest = await getManifest(repo, reference);
	if (!manifest) return null;
	if (Array.isArray(manifest.manifests)) {
		const entry =
			manifest.manifests.find(
				(m: any) => m.platform?.architecture === 'amd64' && m.platform?.os === 'linux'
			) ?? manifest.manifests[0];
		if (!entry) return null;
		return resolveLayers(repo, entry.digest);
	}
	const layers = (manifest.layers ?? []).map((l: any) => ({ digest: l.digest, size: l.size ?? 0 }));
	if (manifest.config?.digest) {
		layers.push({ digest: manifest.config.digest, size: manifest.config.size ?? 0 });
	}
	const totalSize = layers.reduce((sum: number, l: { size: number }) => sum + l.size, 0);
	return { layers, totalSize, configDigest: manifest.config?.digest };
}

async function getCreatedDate(repo: string, configDigest: string | undefined): Promise<string | null> {
	if (!configDigest) return null;
	const config = await getBlob(repo, configDigest);
	return config?.created ?? null;
}

// Per-tag size, plus how much of that size is unique to this tag within the
// repo (i.e. not shared with any other tag) — layers shared across tags (a
// common base image, for instance) are only "owned" by one tag's storage
// footprint when they're actually unique to it.
export const getRepoTagSizes = query(z.string(), async (repo) => {
	const org = resolveOrg();
	if (!repo.startsWith(`${org.slug}/`)) error(403, 'Forbidden');
	const { tags } = await listTags(repo);

	const resolved = await Promise.all(
		tags.map(async (tag) => ({ tag, data: await resolveLayers(repo, tag) }))
	);

	const digestRefCount = new Map<string, number>();
	const digestSize = new Map<string, number>();
	for (const { data } of resolved) {
		if (!data) continue;
		const uniqueDigestsInThisTag = new Set(data.layers.map((l) => l.digest));
		for (const d of uniqueDigestsInThisTag) {
			digestRefCount.set(d, (digestRefCount.get(d) ?? 0) + 1);
		}
		for (const l of data.layers) digestSize.set(l.digest, l.size);
	}

	const tagSizes = await Promise.all(
		resolved.map(async ({ tag, data }) => {
			if (!data) return { tag, totalSize: 0, uniqueSize: 0, createdAt: null };
			let uniqueSize = 0;
			for (const l of new Set(data.layers.map((x) => x.digest))) {
				if (digestRefCount.get(l) === 1) uniqueSize += digestSize.get(l) ?? 0;
			}
			const createdAt = await getCreatedDate(repo, data.configDigest);
			return { tag, totalSize: data.totalSize, uniqueSize, createdAt };
		})
	);

	const totalUniqueBytes = [...digestSize.values()].reduce((a, b) => a + b, 0);
	const naiveSumBytes = tagSizes.reduce((a, t) => a + t.totalSize, 0);

	return { tags: tagSizes, totalUniqueBytes, naiveSumBytes };
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

import { config } from './config';

// Zot registry client using OCI Distribution API v2
// All calls are server-side; Zot is cluster-internal only

async function zotFetch(path: string, options?: RequestInit) {
	const registryInternal = process.env.REGISTRY_INTERNAL_URL ?? 'http://zot.enzarb-system:5000';
	const token = process.env.REGISTRY_ADMIN_TOKEN ?? '';
	const res = await fetch(`${registryInternal}${path}`, {
		...options,
		headers: {
			Authorization: `Bearer ${token}`,
			...options?.headers
		}
	});
	if (!res.ok && res.status !== 404) {
		throw new Error(`Registry error ${res.status}: ${await res.text()}`);
	}
	return res;
}

export interface Repository {
	name: string;
}

export interface TagList {
	name: string;
	tags: string[];
}

export async function listRepositories(): Promise<Repository[]> {
	const res = await zotFetch('/v2/_catalog');
	if (res.status === 404) return [];
	const data = await res.json();
	return (data.repositories ?? []).map((name: string) => ({ name }));
}

export async function listTags(repo: string): Promise<TagList> {
	const res = await zotFetch(`/v2/${repo}/tags/list`);
	if (res.status === 404) return { name: repo, tags: [] };
	return res.json();
}

export async function getManifest(repo: string, reference: string) {
	const res = await zotFetch(`/v2/${repo}/manifests/${reference}`, {
		headers: { Accept: 'application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json' }
	});
	if (res.status === 404) return null;
	return res.json();
}

export async function deleteManifest(repo: string, digest: string) {
	await zotFetch(`/v2/${repo}/manifests/${digest}`, { method: 'DELETE' });
}

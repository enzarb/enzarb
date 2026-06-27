// Zot registry client using OCI Distribution API v2.
// All calls are server-side; Zot is cluster-internal only.
//
// Zot enforces Docker token auth, so every request needs a scoped bearer token
// minted by authd. The app authenticates to authd as "admin" (shared secret)
// and receives a token granting pull/delete on the requested scope.

import { env } from '$env/dynamic/private';

async function registryToken(scope: string): Promise<string> {
	const authd = env.AUTHD_INTERNAL_URL ?? 'http://enzarb-authd.enzarb-system:8080';
	const secret = env.REGISTRY_ADMIN_TOKEN ?? '';
	const params = new URLSearchParams({ service: 'registry.enzarb.dev', scope });
	const res = await fetch(`${authd}/auth/token?${params}`, {
		headers: { Authorization: `Basic ${btoa(`admin:${secret}`)}` }
	});
	if (!res.ok) {
		const body = await res.text();
		console.error(`[authd] token request failed ${res.status}`, { scope, body });
		throw new Error(`authd token error ${res.status}: ${body}`);
	}
	const data = await res.json();
	return data.token ?? data.access_token;
}

async function zotFetch(path: string, scope: string, options?: RequestInit) {
	const registryInternal = env.REGISTRY_INTERNAL_URL ?? 'http://zot.enzarb-system:5000';
	const token = await registryToken(scope);
	const res = await fetch(`${registryInternal}${path}`, {
		...options,
		headers: {
			Authorization: `Bearer ${token}`,
			...options?.headers
		}
	});
	if (!res.ok && res.status !== 404) {
		const body = await res.text();
		const wwwAuth = res.headers.get('WWW-Authenticate') ?? '';
		console.error(`[zot] ${options?.method ?? 'GET'} ${path} → ${res.status}`, { wwwAuth, body });
		throw new Error(`Registry error ${res.status}: ${body}`);
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
	// Zot v2 uses 'repository::pull' (not 'registry:catalog:*') to authorize _catalog.
	const res = await zotFetch('/v2/_catalog', 'repository::pull');
	if (res.status === 404) return [];
	const data = await res.json();
	return (data.repositories ?? []).map((name: string) => ({ name }));
}

export async function listTags(repo: string): Promise<TagList> {
	const res = await zotFetch(`/v2/${repo}/tags/list`, `repository:${repo}:pull`);
	if (res.status === 404) return { name: repo, tags: [] };
	return res.json();
}

export async function getManifest(repo: string, reference: string) {
	const res = await zotFetch(`/v2/${repo}/manifests/${reference}`, `repository:${repo}:pull`, {
		headers: { Accept: 'application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json' }
	});
	if (res.status === 404) return null;
	return res.json();
}

export async function deleteManifest(repo: string, digest: string) {
	await zotFetch(`/v2/${repo}/manifests/${digest}`, `repository:${repo}:pull,delete`, { method: 'DELETE' });
}

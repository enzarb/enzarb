// Zot registry client using OCI Distribution API v2.
// All calls are server-side; Zot is cluster-internal only.
//
// Zot enforces Docker token auth, so every request needs a scoped bearer token
// minted by authd. The app authenticates to authd as "admin" (shared secret)
// and receives a token granting pull/delete on the requested scope.

import { env } from '$env/dynamic/private';

// authd-issued tokens are valid for 5 minutes; cache per-scope so a burst of
// zot calls for the same repo (tag list, per-tag manifests, per-tag config
// blobs) doesn't mint a fresh token on every single request.
const TOKEN_CACHE_TTL_MS = 4 * 60 * 1000;
const tokenCache = new Map<string, { token: string; expiresAt: number }>();
const tokenInflight = new Map<string, Promise<string>>();

async function registryToken(scope: string): Promise<string> {
	const cached = tokenCache.get(scope);
	if (cached && cached.expiresAt > Date.now()) return cached.token;

	const inflight = tokenInflight.get(scope);
	if (inflight) return inflight;

	const promise = (async () => {
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
		const token = data.token ?? data.access_token;
		tokenCache.set(scope, { token, expiresAt: Date.now() + TOKEN_CACHE_TTL_MS });
		return token;
	})();

	tokenInflight.set(scope, promise);
	try {
		return await promise;
	} finally {
		tokenInflight.delete(scope);
	}
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

export async function getBlob(repo: string, digest: string) {
	const res = await zotFetch(`/v2/${repo}/blobs/${digest}`, `repository:${repo}:pull`);
	if (res.status === 404) return null;
	return res.json();
}

export async function getManifest(repo: string, reference: string) {
	const res = await zotFetch(`/v2/${repo}/manifests/${reference}`, `repository:${repo}:pull`, {
		headers: { Accept: 'application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json' }
	});
	if (res.status === 404) return null;
	return res.json();
}

export async function getManifestDigest(repo: string, reference: string): Promise<string | null> {
	const res = await zotFetch(`/v2/${repo}/manifests/${reference}`, `repository:${repo}:pull`, {
		method: 'HEAD',
		headers: { Accept: 'application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json,application/vnd.oci.image.index.v1+json,application/vnd.docker.distribution.manifest.list.v2+json' }
	});
	if (res.status === 404) return null;
	return res.headers.get('Docker-Content-Digest');
}

export async function deleteManifest(repo: string, digest: string) {
	await zotFetch(`/v2/${repo}/manifests/${digest}`, `repository:${repo}:pull,delete`, { method: 'DELETE' });
}

// Delete every tag/manifest in a repository. The registry stores tags as
// references to manifests, and multiple tags can share one manifest digest, so
// we resolve each tag to its digest, dedupe, and delete each unique manifest
// once. Returns the number of manifests deleted.
export async function deleteRepository(repo: string): Promise<number> {
	const { tags } = await listTags(repo);
	const digests = new Set<string>();
	for (const tag of tags) {
		const digest = await getManifestDigest(repo, tag);
		if (digest) digests.add(digest);
	}
	for (const digest of digests) {
		await deleteManifest(repo, digest);
	}
	return digests.size;
}

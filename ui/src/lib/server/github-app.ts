import { createPrivateKey } from 'node:crypto';
import { SignJWT } from 'jose';

export const GITHUB_API = 'https://api.github.com';
const JWT_BACKDATE_S = 60;
const JWT_TTL_S = 9 * 60;
const TIMEOUT_MS = 10_000;

/** RS256 GitHub App JWT (backdated a minute for clock skew, 9 minute lifetime). */
export async function signAppJwt(appId: string, privateKey: string, now: number): Promise<string> {
	const iat = Math.floor(now / 1000) - JWT_BACKDATE_S;
	return new SignJWT({})
		.setProtectedHeader({ alg: 'RS256' })
		.setIssuer(appId)
		.setIssuedAt(iat)
		.setExpirationTime(iat + JWT_TTL_S)
		.sign(createPrivateKey(privateKey));
}

export function githubHeaders(token: string): Record<string, string> {
	return {
		Accept: 'application/vnd.github+json',
		'Content-Type': 'application/json',
		'User-Agent': 'enzarb-ui',
		'X-GitHub-Api-Version': '2022-11-28',
		Authorization: `Bearer ${token}`
	};
}

export interface RepoTokenOptions {
	appId?: string;
	privateKey?: string;
	fetch?: typeof fetch;
	now?: () => number;
}

/** Permissions a repository token may ask for (`contents`, `pull_requests`, `metadata`). */
export type RepoPermissions = Partial<
	Record<'contents' | 'pull_requests' | 'metadata', 'read' | 'write'>
>;

/**
 * An installation token for one repository (`owner/repo`), down-scoped to that repository and to
 * `permissions`. `null` (never a thrown error that could carry key material) when the App is not
 * configured, not installed on the repository, or minting fails.
 */
export async function repoInstallationToken(
	fullName: string,
	permissions: RepoPermissions,
	{ appId, privateKey, fetch: doFetch = fetch, now = Date.now }: RepoTokenOptions
): Promise<string | null> {
	if (!appId || !privateKey) return null;
	const [owner, repo] = fullName.split('/');
	try {
		const jwt = await signAppJwt(appId, privateKey, now());
		const call = (url: string, init: RequestInit) =>
			doFetch(url, {
				...init,
				headers: githubHeaders(jwt),
				redirect: 'error',
				signal: AbortSignal.timeout(TIMEOUT_MS)
			});
		const inst = await call(
			`${GITHUB_API}/repos/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/installation`,
			{ method: 'GET' }
		);
		if (!inst.ok) return null;
		const id = ((await inst.json()) as { id?: unknown }).id;
		if (typeof id !== 'number') return null;
		const res = await call(`${GITHUB_API}/app/installations/${id}/access_tokens`, {
			method: 'POST',
			body: JSON.stringify({ repositories: [repo], permissions })
		});
		if (!res.ok) return null;
		const token = ((await res.json()) as { token?: unknown }).token;
		return typeof token === 'string' && token ? token : null;
	} catch {
		// Deliberately drop the error: it could echo key material.
		return null;
	}
}

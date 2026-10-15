import { createPrivateKey } from 'node:crypto';
import { SignJWT } from 'jose';

export const GITHUB_API = 'https://api.github.com';
const JWT_BACKDATE_S = 60;
const JWT_TTL_S = 9 * 60;
const TIMEOUT_MS = 10_000;
/** Tokens are refreshed this long before they expire. */
export const REPO_TOKEN_MARGIN_MS = 5 * 60_000;
/** How long a repository's installation id is remembered. */
const INSTALLATION_TTL_MS = 60 * 60_000;

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

const cacheKey = (fullName: string, permissions: RepoPermissions) =>
	`${fullName.toLowerCase()}\0${JSON.stringify(Object.entries(permissions).sort())}`;

/**
 * Installation tokens for single repositories (`owner/repo`), down-scoped to that repository and
 * to `permissions`. Installation ids are cached per repository, tokens per (repository,
 * permissions) until shortly before they expire, and concurrent callers share one mint. `get`
 * resolves `null` (never a thrown error that could carry key material) when the App is not
 * configured, not installed on the repository, or minting fails; failures are not cached.
 */
export function createRepoTokenSource({
	appId,
	privateKey,
	fetch: doFetch = fetch,
	now = Date.now
}: RepoTokenOptions) {
	const installations = new Map<string, { id: number; until: number }>();
	const tokens = new Map<string, { token: string; refreshAt: number }>();
	const inflight = new Map<string, Promise<string | null>>();

	async function mint(
		key: string,
		fullName: string,
		permissions: RepoPermissions
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
			const instKey = fullName.toLowerCase();
			let inst = installations.get(instKey);
			if (!inst || inst.until <= now()) {
				const res = await call(
					`${GITHUB_API}/repos/${encodeURIComponent(owner)}/${encodeURIComponent(repo)}/installation`,
					{ method: 'GET' }
				);
				if (!res.ok) return null;
				const id = ((await res.json()) as { id?: unknown }).id;
				if (typeof id !== 'number') return null;
				inst = { id, until: now() + INSTALLATION_TTL_MS };
				installations.set(instKey, inst);
			}
			const res = await call(`${GITHUB_API}/app/installations/${inst.id}/access_tokens`, {
				method: 'POST',
				body: JSON.stringify({ repositories: [repo], permissions })
			});
			if (!res.ok) {
				// The installation may have changed: look it up again next time.
				installations.delete(instKey);
				return null;
			}
			const body = (await res.json()) as { token?: unknown; expires_at?: unknown };
			if (typeof body.token !== 'string' || !body.token) return null;
			const expires =
				typeof body.expires_at === 'string' ? Date.parse(body.expires_at) : Number.NaN;
			// Without a usable expiry assume the 1h GitHub default.
			const end = Number.isFinite(expires) ? expires : now() + 60 * 60_000;
			tokens.set(key, { token: body.token, refreshAt: end - REPO_TOKEN_MARGIN_MS });
			return body.token;
		} catch {
			// Deliberately drop the error: it could echo key material.
			return null;
		}
	}

	return {
		get(fullName: string, permissions: RepoPermissions): Promise<string | null> {
			const key = cacheKey(fullName, permissions);
			const hit = tokens.get(key);
			if (hit && now() < hit.refreshAt) return Promise.resolve(hit.token);
			let p = inflight.get(key);
			if (!p) {
				p = mint(key, fullName, permissions).finally(() => inflight.delete(key));
				inflight.set(key, p);
			}
			return p;
		},
		/** Drop a cached token (GitHub rejected it). */
		invalidate(fullName: string, permissions: RepoPermissions): void {
			tokens.delete(cacheKey(fullName, permissions));
		}
	};
}

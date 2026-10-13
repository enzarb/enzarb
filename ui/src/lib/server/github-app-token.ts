import { createPrivateKey } from 'node:crypto';
import { SignJWT } from 'jose';
import { getConfig } from './config.ts';
import { RELEASES_REPO } from './releases.ts';

const API = 'https://api.github.com';
const JWT_BACKDATE_S = 60;
const JWT_TTL_S = 9 * 60;
export const TOKEN_REFRESH_MARGIN_MS = 5 * 60_000;
const TIMEOUT_MS = 5_000;

export interface TokenSourceOptions {
	appId?: string;
	/** PEM, PKCS#1 or PKCS#8. */
	privateKey?: string;
	fetch?: typeof fetch;
	now?: () => number;
}

interface Cached {
	token: string;
	/** ms epoch after which the token is refreshed. */
	refreshAt: number;
}

/**
 * Server-side GitHub App installation token for `enzarb/enzarb`, cached in memory until shortly
 * before it expires. `get()` resolves `null` (never rejects, never carries the token or key in an
 * error) when the App is unconfigured, not installed on the repo, or minting fails; failures are
 * not cached. Concurrent callers share one in-flight mint.
 */
export function createInstallationTokenSource({
	appId,
	privateKey,
	fetch: doFetch = fetch,
	now = Date.now
}: TokenSourceOptions) {
	let cached: Cached | undefined;
	let inflight: Promise<string | null> | undefined;

	async function call(url: string, init: RequestInit, jwt: string) {
		return doFetch(url, {
			...init,
			headers: {
				Accept: 'application/vnd.github+json',
				'User-Agent': 'enzarb-ui',
				'X-GitHub-Api-Version': '2022-11-28',
				Authorization: `Bearer ${jwt}`
			},
			signal: AbortSignal.timeout(TIMEOUT_MS)
		});
	}

	async function mint(): Promise<string | null> {
		if (!appId || !privateKey) return null;
		try {
			const iat = Math.floor(now() / 1000) - JWT_BACKDATE_S;
			const jwt = await new SignJWT({})
				.setProtectedHeader({ alg: 'RS256' })
				.setIssuer(appId)
				.setIssuedAt(iat)
				.setExpirationTime(iat + JWT_TTL_S)
				.sign(createPrivateKey(privateKey));
			const inst = await call(`${API}/repos/${RELEASES_REPO}/installation`, { method: 'GET' }, jwt);
			if (!inst.ok) return null;
			const id = ((await inst.json()) as { id?: unknown }).id;
			if (typeof id !== 'number' && typeof id !== 'string') return null;
			const res = await call(
				`${API}/app/installations/${encodeURIComponent(String(id))}/access_tokens`,
				{ method: 'POST' },
				jwt
			);
			if (!res.ok) return null;
			const body = (await res.json()) as { token?: unknown; expires_at?: unknown };
			if (typeof body.token !== 'string' || !body.token) return null;
			const expires =
				typeof body.expires_at === 'string' ? Date.parse(body.expires_at) : Number.NaN;
			// Without a usable expiry assume the 1h GitHub default.
			const end = Number.isFinite(expires) ? expires : now() + 60 * 60_000;
			cached = { token: body.token, refreshAt: end - TOKEN_REFRESH_MARGIN_MS };
			return body.token;
		} catch {
			// Deliberately drop the error: it could echo key material.
			return null;
		}
	}

	return {
		get(): Promise<string | null> {
			if (cached && now() < cached.refreshAt) return Promise.resolve(cached.token);
			inflight ??= mint().finally(() => {
				inflight = undefined;
			});
			return inflight;
		},
		/** Drop the cached token (GitHub rejected it). */
		invalidate(): void {
			cached = undefined;
		}
	};
}

let shared: ReturnType<typeof createInstallationTokenSource> | undefined;

const source = () => {
	if (!shared) {
		const { githubAppId, githubAppPrivateKey } = getConfig();
		shared = createInstallationTokenSource({
			appId: githubAppId,
			privateKey: githubAppPrivateKey
		});
	}
	return shared;
};

/** Installation token for the process, or `null` to fetch unauthenticated. Never throws. */
export const getInstallationToken = (): Promise<string | null> => source().get();

/** Forget the process's cached token after GitHub rejected it. */
export const invalidateInstallationToken = (): void => source().invalidate();

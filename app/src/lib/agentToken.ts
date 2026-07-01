import { getAgentToken } from './remote/projects.remote';

// Agent JWTs expire after 5 minutes (see mintProjectToken in jwt.ts). Treat as
// expired 30s early so a check-then-use race doesn't slip past the deadline.
export function isTokenExpired(token: string): boolean {
	try {
		const payload = JSON.parse(atob(token.split('.')[1]));
		return typeof payload.exp === 'number' && payload.exp * 1000 - 30_000 < Date.now();
	} catch {
		return true;
	}
}

// Returns a token guaranteed not to be near-expired, re-minting only when the
// cached one is missing or expiring. Pages that fetch a token once (e.g. on
// mount) and hold onto it were 401ing against the agent once left open past
// the 5-minute lifetime — callers should route every agent fetch through this
// instead of reusing a token captured earlier.
export async function ensureFreshToken(cached: string | null): Promise<string | null> {
	if (cached && !isTokenExpired(cached)) return cached;
	try {
		return await getAgentToken();
	} catch {
		return null;
	}
}

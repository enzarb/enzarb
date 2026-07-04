import { getAgentToken } from './remote/projects.remote';

// Agent JWTs expire after 5 minutes (see mintProjectToken in jwt.ts). Treat as
// expired 30s early so a check-then-use race doesn't slip past the deadline.
function isExpired(token: string): boolean {
	try {
		const payload = JSON.parse(atob(token.split('.')[1]));
		return typeof payload.exp === 'number' && payload.exp * 1000 - 30_000 < Date.now();
	} catch {
		return true;
	}
}

let cached: string | null = null;
let inflight: Promise<string | null> | null = null;

// Single source of truth for the agent JWT — nothing else should hold onto a
// token across awaits. Callers ask here every time they need one; a page left
// open past the 5-minute lifetime just gets a re-mint instead of a stale
// token and a 401. Concurrent callers (e.g. several requests firing at once)
// share one in-flight mint rather than each triggering their own.
export async function getAgentAuthToken(): Promise<string | null> {
	if (cached && !isExpired(cached)) return cached;
	if (!inflight) {
		inflight = (async () => {
			try {
				// getAgentToken is a remote query, so its result is cached on the
				// client: calling it again returns the same (possibly expired)
				// token. refresh() forces a server round-trip to mint a new one.
				const query = getAgentToken();
				let token = await query;
				if (isExpired(token)) {
					await query.refresh();
					token = await query;
				}
				cached = isExpired(token) ? null : token;
				return cached;
			} catch {
				cached = null;
				return null;
			} finally {
				inflight = null;
			}
		})();
	}
	return inflight;
}

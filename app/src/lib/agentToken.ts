import { untrack } from 'svelte';
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

interface Entry {
	token: string | null;
	inflight: Promise<string | null> | null;
}

// Keyed by namespace/project so navigating between projects never serves a
// token minted for a different one.
const cache = new Map<string, Entry>();

// Single source of truth for the agent JWT — nothing else should hold onto a
// token across awaits. Callers ask here every time they need one; a page left
// open past the 5-minute lifetime just gets a re-mint instead of a stale
// token and a 401. Concurrent callers (e.g. several requests firing at once)
// share one in-flight mint rather than each triggering their own.
// getAgentToken is a remote command (never cached client-side), so each mint
// is a real server round-trip.
export async function getAgentAuthToken(
	namespace: string,
	project: string
): Promise<string | null> {
	const key = `${namespace}/${project}`;
	let entry = cache.get(key);
	if (!entry) {
		entry = { token: null, inflight: null };
		cache.set(key, entry);
	}
	if (entry.token && !isExpired(entry.token)) return entry.token;
	if (!entry.inflight) {
		// getAgentToken is a SvelteKit remote command: invoking it synchronously
		// bumps its own $state pending-count. getAgentAuthToken is routinely
		// called from inside a $derived (e.g. the files page's data loader), so
		// that synchronous write trips Svelte's state_unsafe_mutation guard
		// unless it's untracked here, once, for every caller.
		entry.inflight = untrack(() => getAgentToken({ namespace, project }))
			.then((token) => {
				entry.token = token;
				return token;
			})
			.catch(() => {
				entry.token = null;
				return null;
			})
			.finally(() => {
				entry.inflight = null;
			});
	}
	return entry.inflight;
}

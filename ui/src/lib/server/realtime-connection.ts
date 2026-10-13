import { error, type RequestEvent } from '@sveltejs/kit';
import { getConfig } from './config.ts';
import { getProjectStore } from './kube.ts';
import { projectCache } from './project-cache.ts';
import {
	mintRealtimeToken,
	REALTIME_COOKIE,
	REALTIME_TTL_SECONDS,
	realtimeCookieOptions,
	realtimeStreamUrl,
	realtimeUrl
} from './realtime-jwt.ts';

/**
 * Mints a fresh short-lived enzarb-realtime token and delivers it as an HttpOnly cookie, so the
 * page never sees it. Takes no input: the claims come from the session user only.
 */
export async function issueRealtimeConnection({
	locals,
	cookies
}: Pick<RequestEvent, 'locals' | 'cookies'>): Promise<{ url: string; expiresInSeconds: number }> {
	const { user } = locals;
	if (!user) error(401, 'Not signed in');
	const projects = await projectCache.list(getProjectStore(), user.login);
	const config = getConfig();
	// One clock reading: the cookie's Max-Age is the TTL `exp` is built from, so it never outlives the token.
	const nowSeconds = Math.floor(Date.now() / 1000);
	const token = await mintRealtimeToken(
		config.realtimeJwtSecret,
		user.login,
		projects.map((p) => p.name),
		nowSeconds
	);
	cookies.set(REALTIME_COOKIE, token, realtimeCookieOptions());
	// TODO: drop the legacy URL with ENZARB_REALTIME_LEGACY_TOKEN_URL once enzarb-realtime#8 ships.
	const url = config.realtimeLegacyTokenUrl
		? realtimeUrl(config.publicUrl, token)
		: realtimeStreamUrl(config.publicUrl);
	return { url, expiresInSeconds: REALTIME_TTL_SECONDS };
}

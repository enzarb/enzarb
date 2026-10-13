import { SignJWT } from 'jose';

export const REALTIME_AUDIENCE = 'enzarb-realtime';
/** enzarb-realtime accepts tokens for at most 5 minutes (contracts §7a). */
export const REALTIME_TTL_SECONDS = 300;

/** Browser -> enzarb-realtime token: HS256, claims sub/projects/aud/exp. */
export async function mintRealtimeToken(
	secret: string,
	login: string,
	projects: string[],
	nowSeconds = Math.floor(Date.now() / 1000)
): Promise<string> {
	return new SignJWT({ projects })
		.setProtectedHeader({ alg: 'HS256' })
		.setSubject(login)
		.setAudience(REALTIME_AUDIENCE)
		.setIssuedAt(nowSeconds)
		.setExpirationTime(nowSeconds + REALTIME_TTL_SECONDS)
		.sign(new TextEncoder().encode(secret));
}

/** Cookie enzarb-realtime reads the token from; must match the realtime service. */
export const REALTIME_COOKIE = 'enzarb_live';

/** Host-only (no `domain`), always Secure, never readable from JS. */
export function realtimeCookieOptions() {
	return {
		httpOnly: true,
		secure: true,
		sameSite: 'strict',
		path: '/live/',
		maxAge: REALTIME_TTL_SECONDS
	} as const;
}

/** wss://<host>/live/v1/stream, authenticated by the cookie. */
export function realtimeStreamUrl(publicUrl: string): string {
	const u = new URL(publicUrl);
	const scheme = u.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${u.host}/live/v1/stream`;
}

/**
 * wss://<host>/live/v1/stream?token=<jwt>
 * TODO: legacy fallback only; remove with ENZARB_REALTIME_LEGACY_TOKEN_URL once enzarb-realtime#8
 * is released.
 */
export function realtimeUrl(publicUrl: string, token: string): string {
	const u = new URL(publicUrl);
	const scheme = u.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${u.host}/live/v1/stream?token=${encodeURIComponent(token)}`;
}

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

/** wss://<host>/live/v1/stream?token=<jwt> */
export function realtimeUrl(publicUrl: string, token: string): string {
	const u = new URL(publicUrl);
	const scheme = u.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${u.host}/live/v1/stream?token=${encodeURIComponent(token)}`;
}

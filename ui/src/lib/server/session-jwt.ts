import { SignJWT } from 'jose';

export const JWT_AUDIENCE = 'enzarb-sandbox-proxy';
/** contracts §7: exp is at most 5 minutes after issue, and only checked at connect. */
export const JWT_TTL_SECONDS = 60;

/** Browser -> sandbox-proxy token (contracts §7): HS256, claims sub/project/aud/exp. */
export async function mintSessionToken(
	secret: string,
	login: string,
	project: string,
	nowSeconds = Math.floor(Date.now() / 1000)
): Promise<string> {
	return new SignJWT({ project })
		.setProtectedHeader({ alg: 'HS256' })
		.setSubject(login)
		.setAudience(JWT_AUDIENCE)
		.setIssuedAt(nowSeconds)
		.setExpirationTime(nowSeconds + JWT_TTL_SECONDS)
		.sign(new TextEncoder().encode(secret));
}

/** wss://<host>/sandbox/v1/projects/<project>/mux?token=<jwt> (contracts §7). */
export function muxUrl(publicUrl: string, project: string, token: string): string {
	const u = new URL(publicUrl);
	const scheme = u.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${u.host}/sandbox/v1/projects/${encodeURIComponent(project)}/mux?token=${encodeURIComponent(token)}`;
}

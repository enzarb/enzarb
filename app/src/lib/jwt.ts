import { SignJWT, exportJWK, generateKeyPair, importPKCS8, importSPKI } from 'jose';
import { randomUUID } from 'crypto';
import { env } from '$env/dynamic/private';
import { sql } from '$lib/db';

// Key pair loaded once at startup from env or generated for dev
let privateKey: CryptoKey;
let publicKey: CryptoKey;
let publicJwk: Record<string, unknown>;

export async function initKeys() {
	const privateKeyPem = env.JWT_PRIVATE_KEY;
	const publicKeyPem = env.JWT_PUBLIC_KEY;

	if (privateKeyPem && publicKeyPem) {
		privateKey = await importPKCS8(privateKeyPem, 'RS256');
		publicKey = await importSPKI(publicKeyPem, 'RS256');
	} else {
		// Dev mode: generate ephemeral key pair
		const pair = await generateKeyPair('RS256');
		privateKey = pair.privateKey;
		publicKey = pair.publicKey;
	}

	const jwk = await exportJWK(publicKey);
	publicJwk = { ...jwk, kid: 'enzarb-1', alg: 'RS256', use: 'sig' };
}

export function getJwks() {
	return { keys: [publicJwk] };
}

export async function mintProjectToken(
	userId: string,
	projectId: string,
	permissions: string[]
): Promise<string> {
	const jti = randomUUID();
	const token = await new SignJWT({
		sub: userId,
		projects: { [projectId]: permissions },
		aud: 'enzarb-agent'
	})
		.setProtectedHeader({ alg: 'RS256', kid: 'enzarb-1' })
		.setIssuedAt()
		.setJti(jti)
		.setExpirationTime('5m')
		.setIssuer('https://enzarb.dev')
		.sign(privateKey);
	return token;
}

export async function revokeToken(jti: string, expiresAt: Date): Promise<void> {
	await sql`
		INSERT INTO jwt_revocations (jti, expires_at) VALUES (${jti}, ${expiresAt})
		ON CONFLICT (jti) DO NOTHING
	`;
}

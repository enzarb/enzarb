import { SignJWT, exportJWK, generateKeyPair, importPKCS8, importSPKI } from 'jose';
import type { KeyLike } from 'jose';

// Key pair loaded once at startup from env or generated for dev
let privateKey: KeyLike;
let publicKey: KeyLike;
let publicJwk: Record<string, string>;

export async function initKeys() {
	const privateKeyPem = process.env.JWT_PRIVATE_KEY;
	const publicKeyPem = process.env.JWT_PUBLIC_KEY;

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
	publicJwk = { ...jwk, kid: 'enzarb-1', alg: 'RS256', use: 'sig' } as Record<string, string>;
}

export function getJwks() {
	return { keys: [publicJwk] };
}

export async function mintProjectToken(
	userId: string,
	projectId: string,
	permissions: string[]
): Promise<string> {
	return new SignJWT({
		sub: userId,
		projects: { [projectId]: permissions },
		aud: 'enzarb-agent'
	})
		.setProtectedHeader({ alg: 'RS256', kid: 'enzarb-1' })
		.setIssuedAt()
		.setExpirationTime('30s')
		.setIssuer('https://enzarb.dev')
		.sign(privateKey);
}

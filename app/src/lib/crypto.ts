import { createCipheriv, createDecipheriv, randomBytes } from 'crypto';
import { env } from '$env/dynamic/private';

const ALGO = 'aes-256-gcm';
const PREFIX = 'enc:v1:';

function getKey(): Buffer {
	const hex = env.ENCRYPTION_KEY ?? '';
	if (!hex) throw new Error('ENCRYPTION_KEY is not set');
	const key = Buffer.from(hex, 'hex');
	if (key.length !== 32) throw new Error('ENCRYPTION_KEY must be 32 bytes (64 hex chars)');
	return key;
}

export function encrypt(plaintext: string): string {
	const key = getKey();
	const iv = randomBytes(12);
	const cipher = createCipheriv(ALGO, key, iv);
	const encrypted = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()]);
	const authTag = cipher.getAuthTag();
	return `${PREFIX}${iv.toString('hex')}:${authTag.toString('hex')}:${encrypted.toString('hex')}`;
}

export function decrypt(value: string): string {
	// Backward compat: plain values (before encryption was introduced) pass through.
	if (!value.startsWith(PREFIX)) return value;

	const parts = value.slice(PREFIX.length).split(':');
	if (parts.length !== 3) throw new Error('Invalid encrypted value format');
	const [ivHex, authTagHex, dataHex] = parts;

	const key = getKey();
	const iv = Buffer.from(ivHex, 'hex');
	const authTag = Buffer.from(authTagHex, 'hex');
	const data = Buffer.from(dataHex, 'hex');

	const decipher = createDecipheriv(ALGO, key, iv);
	decipher.setAuthTag(authTag);
	return decipher.update(data) + decipher.final('utf8');
}

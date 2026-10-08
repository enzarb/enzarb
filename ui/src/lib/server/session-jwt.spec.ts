import { jwtVerify } from 'jose';
import { describe, expect, it } from 'vitest';
import { JWT_AUDIENCE, JWT_TTL_SECONDS, mintSessionToken, muxUrl } from './session-jwt.ts';

const key = new TextEncoder().encode('s3cret');

describe('mintSessionToken', () => {
	it('signs HS256 with sub, project, aud and a short exp', async () => {
		const now = 1_700_000_000;
		const token = await mintSessionToken('s3cret', 'octocat', 'demo', now);
		const { payload, protectedHeader } = await jwtVerify(token, key, {
			audience: JWT_AUDIENCE,
			currentDate: new Date(now * 1000)
		});
		expect(protectedHeader.alg).toBe('HS256');
		expect(payload.sub).toBe('octocat');
		expect(payload.project).toBe('demo');
		expect(payload.exp).toBe(now + JWT_TTL_SECONDS);
		expect(JWT_TTL_SECONDS).toBeLessThanOrEqual(300);
	});

	it('is rejected with the wrong secret', async () => {
		const token = await mintSessionToken('s3cret', 'octocat', 'demo');
		await expect(jwtVerify(token, new TextEncoder().encode('other'))).rejects.toThrow();
	});
});

describe('muxUrl', () => {
	it('maps http(s) to ws(s) and targets the proxy path', () => {
		expect(muxUrl('https://enzarb.example.com', 'demo', 'a.b')).toBe(
			'wss://enzarb.example.com/sandbox/v1/projects/demo/mux?token=a.b'
		);
		expect(muxUrl('http://localhost:5173', 'demo', 't')).toBe(
			'ws://localhost:5173/sandbox/v1/projects/demo/mux?token=t'
		);
	});
});

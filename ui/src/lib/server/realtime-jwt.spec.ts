import { jwtVerify } from 'jose';
import { describe, expect, it } from 'vitest';
import {
	mintRealtimeToken,
	REALTIME_AUDIENCE,
	REALTIME_TTL_SECONDS,
	realtimeCookieOptions,
	realtimeStreamUrl,
	realtimeUrl
} from './realtime-jwt.ts';

const key = new TextEncoder().encode('s3cret');

describe('mintRealtimeToken', () => {
	it('signs HS256 with sub, projects, aud and exp within 5 minutes', async () => {
		const now = 1_700_000_000;
		const token = await mintRealtimeToken('s3cret', 'octocat', ['a', 'b'], ['o/a'], now);
		const { payload, protectedHeader } = await jwtVerify(token, key, {
			audience: REALTIME_AUDIENCE,
			currentDate: new Date(now * 1000)
		});
		expect(protectedHeader.alg).toBe('HS256');
		expect(payload.sub).toBe('octocat');
		expect(payload.projects).toEqual(['a', 'b']);
		expect(payload.repos).toEqual(['o/a']);
		expect(payload.iat).toBe(now);
		expect(payload.exp).toBe(now + REALTIME_TTL_SECONDS);
		expect(REALTIME_TTL_SECONDS).toBeLessThanOrEqual(300);
	});

	it('does not verify with another secret', async () => {
		const token = await mintRealtimeToken('s3cret', 'octocat', [], []);
		await expect(jwtVerify(token, new TextEncoder().encode('other'))).rejects.toThrow();
	});
});

describe('realtimeCookieOptions', () => {
	it('is HttpOnly, Secure, SameSite=Strict, Path=/live/ and host-only', () => {
		const o = realtimeCookieOptions();
		expect(o).toMatchObject({
			httpOnly: true,
			secure: true,
			sameSite: 'strict',
			path: '/live/',
			maxAge: 300
		});
		expect('domain' in o).toBe(false);
	});

	it('expires with the token', async () => {
		const now = 1_700_000_000;
		const token = await mintRealtimeToken('s3cret', 'octocat', [], [], now);
		const { payload } = await jwtVerify(token, key, { currentDate: new Date(now * 1000) });
		expect(realtimeCookieOptions().maxAge).toBe(payload.exp! - payload.iat!);
	});
});

describe('realtimeStreamUrl', () => {
	it('has no query string', () => {
		expect(realtimeStreamUrl('https://enzarb.example.com')).toBe(
			'wss://enzarb.example.com/live/v1/stream'
		);
		expect(realtimeStreamUrl('http://localhost:5173')).toBe('ws://localhost:5173/live/v1/stream');
	});
});

describe('realtimeUrl', () => {
	it('maps http(s) to ws(s) and targets the stream path', () => {
		expect(realtimeUrl('https://enzarb.example.com', 'a.b')).toBe(
			'wss://enzarb.example.com/live/v1/stream?token=a.b'
		);
		expect(realtimeUrl('http://localhost:5173', 't')).toBe(
			'ws://localhost:5173/live/v1/stream?token=t'
		);
	});
});

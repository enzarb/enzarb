import { jwtVerify } from 'jose';
import { describe, expect, it } from 'vitest';
import {
	mintRealtimeToken,
	REALTIME_AUDIENCE,
	REALTIME_TTL_SECONDS,
	realtimeUrl
} from './realtime-jwt.ts';

const key = new TextEncoder().encode('s3cret');

describe('mintRealtimeToken', () => {
	it('signs HS256 with sub, projects, aud and exp within 5 minutes', async () => {
		const now = 1_700_000_000;
		const token = await mintRealtimeToken('s3cret', 'octocat', ['a', 'b'], now);
		const { payload, protectedHeader } = await jwtVerify(token, key, {
			audience: REALTIME_AUDIENCE,
			currentDate: new Date(now * 1000)
		});
		expect(protectedHeader.alg).toBe('HS256');
		expect(payload.sub).toBe('octocat');
		expect(payload.projects).toEqual(['a', 'b']);
		expect(payload.iat).toBe(now);
		expect(payload.exp).toBe(now + REALTIME_TTL_SECONDS);
		expect(REALTIME_TTL_SECONDS).toBeLessThanOrEqual(300);
	});

	it('does not verify with another secret', async () => {
		const token = await mintRealtimeToken('s3cret', 'octocat', []);
		await expect(jwtVerify(token, new TextEncoder().encode('other'))).rejects.toThrow();
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

import * as v from 'valibot';
import { describe, expect, it } from 'vitest';
import {
	BODY_MAX,
	CategoriesSchema,
	SubscriptionSchema,
	clearPayload,
	pushPayload,
	summaryPayload,
	testPayload,
	urlBase64ToUint8Array
} from './push.ts';

describe('pushPayload', () => {
	it('has exactly title, body, url and tag', () => {
		const p = pushPayload({ id: 12, title: 'Add retries', reason: 'Plan awaiting approval' });
		expect(Object.keys(p).sort()).toEqual(['body', 'tag', 'title', 'url']);
		expect(p.title).toBe('Plan awaiting approval');
	});

	it('links to the highlighted card and replaces repeats through the tag', () => {
		const p = pushPayload({ id: 12, title: 'Add retries' });
		expect(p.url).toBe('/tasks?highlight=12');
		expect(p.tag).toBe('item-12');
		expect(p.title).toBe('A task is waiting on you');
		expect(clearPayload(12)).toEqual({ type: 'clear', tag: 'item-12' });
		expect(p.body).toBe('Add retries');
	});

	it('truncates and flattens the body', () => {
		const p = pushPayload({ id: 1, title: `line one\n\n${'x'.repeat(500)}` });
		expect(p.body.length).toBeLessThanOrEqual(BODY_MAX);
		expect(p.body.endsWith('…')).toBe(true);
		expect(p.body).not.toContain('\n');
	});

	it('is built from the id, title and reason only', () => {
		const item = { id: 5, title: 'Fix it', reason: 'Review', stateData: { token: 'ghp_abc' } };
		const json = JSON.stringify(pushPayload(item));
		for (const secret of ['ghp_abc', 'stateData', 'token']) expect(json).not.toContain(secret);
	});
});

describe('other payloads', () => {
	it('collapses a burst into one tagged summary', () => {
		const p = summaryPayload(7);
		expect(p.title).toBe('7 tasks need you');
		expect(p.url).toBe('/tasks');
		expect(p.tag).toBe('work-summary');
		expect(Object.keys(p).sort()).toEqual(['body', 'tag', 'title', 'url']);
	});

	it('keeps the test notification in-app', () => {
		expect(testPayload().url.startsWith('/')).toBe(true);
	});
});

describe('schemas', () => {
	const sub = { endpoint: 'https://push.example/abc', keys: { p256dh: 'k', auth: 'a' } };

	it('accepts only known categories', () => {
		expect(v.safeParse(CategoriesSchema, ['needs_you']).success).toBe(true);
		// devices subscribed before ADR-0022 keep their old names
		expect(v.safeParse(CategoriesSchema, ['approval', 'merge']).success).toBe(true);
		expect(v.safeParse(CategoriesSchema, ['everything']).success).toBe(false);
	});

	it('requires an https endpoint and both keys', () => {
		const ok = { ...sub, categories: ['failed'] };
		expect(v.safeParse(SubscriptionSchema, ok).success).toBe(true);
		expect(
			v.safeParse(SubscriptionSchema, { ...ok, endpoint: 'http://push.example/x' }).success
		).toBe(false);
		expect(v.safeParse(SubscriptionSchema, { ...ok, keys: { p256dh: 'k' } }).success).toBe(false);
	});
});

describe('urlBase64ToUint8Array', () => {
	it('decodes base64url without padding', () => {
		expect([...urlBase64ToUint8Array('_-8')]).toEqual([0xff, 0xef]);
	});
});

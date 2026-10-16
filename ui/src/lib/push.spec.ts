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
import { NEEDS_YOU_CATEGORIES } from './work-items.ts';

describe('pushPayload', () => {
	it('has exactly title, body, url and tag', () => {
		for (const category of NEEDS_YOU_CATEGORIES) {
			const p = pushPayload({ id: 12, title: 'Add retries' }, category);
			expect(Object.keys(p).sort()).toEqual(['body', 'tag', 'title', 'url']);
		}
	});

	it('links to the highlighted card on the board and replaces repeats through the tag', () => {
		const p = pushPayload({ id: 12, title: 'Add retries' }, 'merge');
		expect(p.url).toBe('/work?highlight=12&was=merge');
		expect(p.tag).toBe('item-12');
		expect(pushPayload({ id: 3, title: 'x', project: 'a b' }, 'failed').url).toBe(
			'/work?project=a+b&highlight=3&was=failed'
		);
		expect(clearPayload(12)).toEqual({ type: 'clear', tag: 'item-12' });
		expect(p.body).toBe('Add retries');
	});

	it('uses a fixed title per category and never puts item text in it', () => {
		const titles = NEEDS_YOU_CATEGORIES.map(
			(c) => pushPayload({ id: 1, title: 'secret-ish item title' }, c).title
		);
		expect(new Set(titles).size).toBe(NEEDS_YOU_CATEGORIES.length);
		for (const t of titles) expect(t).not.toContain('secret-ish');
	});

	it('truncates and flattens the body', () => {
		const p = pushPayload({ id: 1, title: `line one\n\n${'x'.repeat(500)}` }, 'failed');
		expect(p.body.length).toBeLessThanOrEqual(BODY_MAX);
		expect(p.body.endsWith('…')).toBe(true);
		expect(p.body).not.toContain('\n');
	});

	it('is built from the id and title only: nothing else of an item can leak', () => {
		const item = {
			id: 5,
			title: 'Fix it',
			stateData: { token: 'ghp_abc', error: 'boom' },
			reason: 'CI: boom'
		};
		const json = JSON.stringify(pushPayload(item, 'blocked'));
		for (const secret of ['ghp_abc', 'boom', 'stateData', 'token'])
			expect(json).not.toContain(secret);
	});
});

describe('other payloads', () => {
	it('collapses a burst into one tagged summary', () => {
		const p = summaryPayload(7);
		expect(p.title).toBe('7 tasks need you');
		expect(p.url).toBe('/work');
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

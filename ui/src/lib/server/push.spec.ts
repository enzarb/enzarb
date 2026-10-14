import { beforeEach, describe, expect, it, vi } from 'vitest';

const sendNotification = vi.hoisted(() => vi.fn());
vi.mock('web-push', () => ({ default: { sendNotification } }));

import { createSend, deliver } from './push.ts';

const vapid = { publicKey: 'pub', privateKey: 'PRIVATE-KEY', subject: 'mailto:ops@example.com' };
const target = { endpoint: 'https://push.example/abc', p256dh: 'k', auth: 'a' };
const payload = { title: 'T', body: 'B', url: '/work/1', tag: 'work-1' };

beforeEach(() => sendNotification.mockReset());

describe('createSend', () => {
	it('posts the JSON payload with the VAPID details', async () => {
		sendNotification.mockResolvedValue({});
		expect(await createSend(vapid)(target, payload)).toBe('ok');
		const [sub, body, options] = sendNotification.mock.calls[0];
		expect(sub).toEqual({ endpoint: target.endpoint, keys: { p256dh: 'k', auth: 'a' } });
		expect(JSON.parse(body)).toEqual(payload);
		expect(options.vapidDetails).toEqual(vapid);
	});

	it('does not put the private key in the message', async () => {
		sendNotification.mockResolvedValue({});
		await createSend(vapid)(target, payload);
		expect(sendNotification.mock.calls[0][1]).not.toContain('PRIVATE-KEY');
	});

	it('reports 404 and 410 as gone and other failures as errors', async () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		for (const [statusCode, want] of [
			[404, 'gone'],
			[410, 'gone'],
			[429, 'error'],
			[500, 'error']
		] as const) {
			sendNotification.mockRejectedValueOnce(Object.assign(new Error('x'), { statusCode }));
			expect(await createSend(vapid)(target, payload)).toBe(want);
		}
		sendNotification.mockRejectedValueOnce(new Error('network'));
		expect(await createSend(vapid)(target, payload)).toBe('error');
		// the capability URL is never logged
		expect(warn.mock.calls.flat().join(' ')).not.toContain('push.example');
		warn.mockRestore();
	});
});

describe('deliver', () => {
	it('prunes gone devices, touches working ones and keeps failing ones', async () => {
		const store = { remove: vi.fn(async () => true), touch: vi.fn(async () => {}) };
		const targets = ['ok', 'gone', 'error'].map((s) => ({ ...target, endpoint: `https://p/${s}` }));
		const send = vi.fn(async (t: { endpoint: string }) => t.endpoint.split('/').pop() as 'ok');
		const n = await deliver(store, send, 'octo', targets, payload);
		expect(n).toBe(1);
		expect(store.remove).toHaveBeenCalledExactlyOnceWith('octo', 'https://p/gone');
		expect(store.touch).toHaveBeenCalledExactlyOnceWith('octo', 'https://p/ok');
	});
});

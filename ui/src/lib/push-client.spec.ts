import { describe, expect, it } from 'vitest';
import { detectSupport, type PushEnv } from './push-client.ts';

const full: PushEnv = {
	hasServiceWorker: true,
	hasPushManager: true,
	hasNotification: true,
	permission: 'default',
	isApple: false,
	standalone: false
};

describe('detectSupport', () => {
	it('is ok when push is available', () => {
		expect(detectSupport(full)).toBe('ok');
		expect(detectSupport({ ...full, permission: 'granted' })).toBe('ok');
	});

	it('reports blocked notifications', () => {
		expect(detectSupport({ ...full, permission: 'denied' })).toBe('denied');
	});

	it('asks iOS users to add the app to the home screen first', () => {
		const ios = { ...full, isApple: true, hasPushManager: false };
		expect(detectSupport(ios)).toBe('needs-install');
		expect(detectSupport({ ...ios, standalone: true })).toBe('unsupported');
		expect(detectSupport({ ...full, isApple: true, standalone: true })).toBe('ok');
	});

	it('is unsupported without a service worker or PushManager elsewhere', () => {
		expect(detectSupport({ ...full, hasServiceWorker: false })).toBe('unsupported');
		expect(detectSupport({ ...full, hasPushManager: false })).toBe('unsupported');
	});
});

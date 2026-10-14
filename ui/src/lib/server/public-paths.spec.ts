import { describe, expect, it } from 'vitest';
import { isPublicPath } from './public-paths.ts';

describe('isPublicPath', () => {
	it('keeps login and the probes public', () => {
		for (const dev of [true, false]) {
			expect(isPublicPath('/login', dev)).toBe(true);
			expect(isPublicPath('/login/github/callback', dev)).toBe(true);
			expect(isPublicPath('/healthz', dev)).toBe(true);
			expect(isPublicPath('/readyz', dev)).toBe(true);
			expect(isPublicPath('/offline', dev)).toBe(true); // static page the service worker caches
			expect(isPublicPath('/projects', dev)).toBe(false);
		}
	});

	it('exempts the component gallery from the auth redirect in dev only', () => {
		expect(isPublicPath('/dev/ui', true)).toBe(true);
		expect(isPublicPath('/dev/ui', false)).toBe(false);
		expect(isPublicPath('/development', true)).toBe(false);
	});
});

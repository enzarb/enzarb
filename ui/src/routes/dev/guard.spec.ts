import { beforeEach, describe, expect, it, vi } from 'vitest';

const env = vi.hoisted(() => ({ dev: true }));
vi.mock('$app/env', () => env);

import { load } from './+layout.server.ts';

describe('/dev guard', () => {
	beforeEach(() => {
		env.dev = true;
	});

	it('serves developer pages in dev', () => {
		expect(() => load({} as never)).not.toThrow();
	});

	it('answers 404 in a production build', () => {
		env.dev = false;
		try {
			load({} as never);
			expect.unreachable();
		} catch (e) {
			expect(e).toMatchObject({ status: 404 });
		}
	});
});

import { describe, expect, it } from 'vitest';
import { load } from './+page.server.ts';

const run = (search: string, name = 'alpha') =>
	(load as (e: unknown) => unknown)({
		url: new URL(`http://localhost/projects/${name}${search}`),
		params: { name }
	});
const redirectOf = (search: string, name?: string) => {
	try {
		run(search, name);
	} catch (e) {
		return e as { status: number; location: string };
	}
};

describe('legacy ?tab= redirect', () => {
	it('does nothing without a tab', () => {
		expect(redirectOf('')).toBeUndefined();
	});

	it.each(['terminal', 'settings'])('redirects ?tab=%s to the tab route (308)', (tab) => {
		expect(redirectOf(`?tab=${tab}`)).toMatchObject({
			status: 308,
			location: `/projects/alpha/${tab}`
		});
	});

	it.each(['overview', 'nope', ''])('sends ?tab=%s to Overview', (tab) => {
		expect(redirectOf(`?tab=${tab}`)).toMatchObject({ status: 308, location: '/projects/alpha' });
	});

	it('keeps other query parameters', () => {
		expect(redirectOf('?tab=settings&x=1')).toMatchObject({
			location: '/projects/alpha/settings?x=1'
		});
	});
});

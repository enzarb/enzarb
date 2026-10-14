import { describe, expect, it } from 'vitest';
import { load } from './+page.server.ts';

const run = (id: string, search = '') =>
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	(load as (e: unknown) => any)({
		params: { id },
		url: new URL(`https://enzarb.test/work/${id}${search}`)
	});

describe('/work/[id] redirect', () => {
	it('permanently redirects to /task/[id]', () => {
		try {
			run('12');
			expect.unreachable();
		} catch (e) {
			expect(e).toMatchObject({ status: 308, location: '/task/12' });
		}
	});

	it('keeps the query string', () => {
		try {
			run('12', '?tab=jobs');
			expect.unreachable();
		} catch (e) {
			expect(e).toMatchObject({ status: 308, location: '/task/12?tab=jobs' });
		}
	});
});

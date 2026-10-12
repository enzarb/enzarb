import { isHttpError } from '@sveltejs/kit';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const store = vi.hoisted(() => ({
	get: vi.fn(async (name: string) =>
		name === 'alpha' ? { metadata: { name }, spec: { owner: 'octo' } } : null
	)
}));
vi.mock('#lib/server/kube.ts', () => ({ getProjectStore: () => store }));
vi.mock('#lib/server/project-service.ts', () => ({
	projectService: (s: typeof store, owner: string) => ({
		get: async (name: string) => {
			const p = await s.get(name);
			return p && p.spec.owner === owner ? { name } : null;
		}
	})
}));

import { load } from './+page.server.ts';

const octo = { id: 1, login: 'octo', name: null };
const mallory = { id: 2, login: 'mallory', name: null };
const run = (user: unknown, name = 'alpha') =>
	(load as (e: unknown) => Promise<unknown>)({ locals: { user }, params: { name } });
const status = async (p: Promise<unknown>) => {
	try {
		await p;
		return 200;
	} catch (e) {
		if (isHttpError(e)) return e.status;
		throw e;
	}
};

describe('chat route', () => {
	beforeEach(() => store.get.mockClear());

	it('renders for the owner', async () => {
		expect(await status(run(octo))).toBe(200);
	});

	it('404s for a project the user does not own or that does not exist', async () => {
		expect(await status(run(mallory))).toBe(404);
		expect(await status(run(octo, 'beta'))).toBe(404);
	});

	it('refuses unauthenticated requests', async () => {
		expect(await status(run(null))).toBe(401);
	});

	it('does not ask the cluster about names that are not DNS labels', async () => {
		for (const name of ['../alpha', 'alpha/x', 'Alpha', '', 'a'.repeat(64), 'alpha?x=1'])
			expect(await status(run(octo, name))).toBe(404);
		expect(store.get).not.toHaveBeenCalled();
	});
});

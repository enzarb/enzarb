import { isHttpError } from '@sveltejs/kit';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

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
import { load as layoutLoad } from '../+layout.server.ts';

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
	afterEach(() => vi.unstubAllEnvs());

	it('404s for everyone, owner included, when the flag is off', async () => {
		for (const v of [undefined, '', 'false', '1', 'TRUE']) {
			if (v === undefined) delete process.env.ENZARB_UI_CHAT;
			else vi.stubEnv('ENZARB_UI_CHAT', v);
			expect(await status(run(octo))).toBe(404);
		}
		expect(store.get).not.toHaveBeenCalled();
	});

	it('renders for the owner when the flag is on', async () => {
		vi.stubEnv('ENZARB_UI_CHAT', 'true');
		expect(await status(run(octo))).toBe(200);
	});

	it('404s for a project the user does not own or that does not exist', async () => {
		vi.stubEnv('ENZARB_UI_CHAT', 'true');
		expect(await status(run(mallory))).toBe(404);
		expect(await status(run(octo, 'beta'))).toBe(404);
	});

	it('refuses unauthenticated requests', async () => {
		vi.stubEnv('ENZARB_UI_CHAT', 'true');
		expect(await status(run(null))).toBe(401);
	});

	it('does not ask the cluster about names that are not DNS labels', async () => {
		vi.stubEnv('ENZARB_UI_CHAT', 'true');
		for (const name of ['../alpha', 'alpha/x', 'Alpha', '', 'a'.repeat(64), 'alpha?x=1'])
			expect(await status(run(octo, name))).toBe(404);
		expect(store.get).not.toHaveBeenCalled();
	});
});

describe('project layout data', () => {
	afterEach(() => vi.unstubAllEnvs());

	it('exposes the flag, off unless exactly true', async () => {
		const data = () => (layoutLoad as () => { chatEnabled: boolean })();
		vi.stubEnv('ENZARB_UI_CHAT', 'yes');
		expect(data().chatEnabled).toBe(false);
		vi.stubEnv('ENZARB_UI_CHAT', 'true');
		expect(data().chatEnabled).toBe(true);
	});
});

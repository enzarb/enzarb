import { isHttpError } from '@sveltejs/kit';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { buildProject } from '#lib/projects.ts';
import type { ProjectStore } from '#lib/server/kube.ts';

// Only the cluster store is faked: the real projectService does the owner check.
const items = vi.hoisted(() => new Map<string, unknown>());
const storeGet = vi.hoisted(() => vi.fn());
vi.mock('#lib/server/kube.ts', () => ({
	getProjectStore: () => ({ get: storeGet }) as unknown as ProjectStore
}));

import { load } from './+page.server.ts';

const project = (name: string, owner: string): Project =>
	buildProject(
		{
			name,
			displayName: '',
			repositories: [],
			triggers: { issues: false, changeRequests: false, mentions: false }
		},
		owner
	);
items.set('alpha', project('alpha', 'octo'));
beforeEach(() => {
	storeGet.mockReset();
	storeGet.mockImplementation(async (name: string) => items.get(name) ?? null);
});

const user = (login: string) => ({ id: 1, login, name: null });
const run = (login: string, name: string) =>
	(load as (e: unknown) => Promise<unknown>)({ locals: { user: user(login) }, params: { name } });
const status = async (p: Promise<unknown>) => {
	try {
		await p;
		return 200;
	} catch (e) {
		if (isHttpError(e)) return e.status;
		throw e;
	}
};

describe('chat route ownership (real projectService)', () => {
	it('lets the owner in', async () => {
		expect(await status(run('octo', 'alpha'))).toBe(200);
	});

	it('404s for another user, the same as for a missing project', async () => {
		expect(await status(run('mallory', 'alpha'))).toBe(404);
	});

	it('404s for a missing project', async () => {
		expect(await status(run('octo', 'beta'))).toBe(404);
	});

	it.each(['../alpha', 'Alpha', 'a/b', 'alpha%2f', '', 'a'.repeat(64)])(
		'404s for the injected or invalid name %j without asking the store',
		async (name) => {
			expect(await status(run('octo', name))).toBe(404);
			expect(storeGet).not.toHaveBeenCalled();
		}
	);
});

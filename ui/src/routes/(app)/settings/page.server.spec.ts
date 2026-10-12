import { describe, expect, it, vi } from 'vitest';

const snapshot = vi.hoisted(() => ({
	value: { repositories: [] as unknown[], fetchedAt: null as Date | null }
}));
vi.mock('#lib/server/db.ts', () => ({ getSql: () => ({}) }));
vi.mock('#lib/server/config.ts', () => ({ getConfig: () => ({ githubAppSlug: 'my-app' }) }));
vi.mock('#lib/server/user-repositories.ts', async (orig) => ({
	...(await orig<typeof import('#lib/server/user-repositories.ts')>()),
	getUserRepositorySnapshot: async () => snapshot.value
}));

import { load } from './+page.server.ts';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const run = (user: unknown) => (load as (e: unknown) => Promise<any>)({ locals: { user } });
const user = { id: 1, login: 'octo', name: null };
const row = (repo: string, accountLogin: string, installationId: number) => ({
	repo,
	accountLogin,
	installationId,
	private: false,
	defaultBranch: 'main'
});

describe('settings load', () => {
	it('groups rows by account and builds the install url', async () => {
		snapshot.value = {
			repositories: [
				row('github:b/x', 'b', 2),
				row('github:a/x', 'a', 1),
				row('github:a/y', 'a', 1)
			],
			fetchedAt: new Date('2026-10-11T10:00:00Z')
		};
		const data = await run(user);
		expect(data.installations).toEqual([
			{ accountLogin: 'a', installationId: 1, repositoryCount: 2 },
			{ accountLogin: 'b', installationId: 2, repositoryCount: 1 }
		]);
		expect(data.fetchedAt).toBe('2026-10-11T10:00:00.000Z');
		expect(data.installUrl).toBe('https://github.com/apps/my-app/installations/new');
		expect(data.account.login).toBe('octo');
	});

	it('handles an empty snapshot', async () => {
		snapshot.value = { repositories: [], fetchedAt: null };
		const data = await run(user);
		expect(data.installations).toEqual([]);
		expect(data.fetchedAt).toBeNull();
	});

	it('redirects when signed out', async () => {
		await expect(run(null)).rejects.toMatchObject({ status: 303 });
	});
});

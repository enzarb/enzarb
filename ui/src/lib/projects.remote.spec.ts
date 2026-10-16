import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import type { UserRepository } from '#lib/server/user-repositories.ts';

const h = vi.hoisted(() => ({
	configured: true,
	listed: [] as UserRepository[],
	listFails: false,
	listCalls: [] as { login: string; refresh: boolean }[],
	allowed: new Set<string>(),
	checks: [] as [string, string][],
	queryRefreshes: 0,
	projects: [] as Project[],
	updates: [] as unknown[]
}));

vi.mock('$app/server', () => ({
	command: (a: unknown, b?: unknown) =>
		Object.assign((b ?? a) as object, { __: { type: 'command' } }),
	query: (a: unknown, b?: unknown) => {
		const fn = (b ?? a) as (...args: unknown[]) => Promise<unknown>;
		const wrapped = (...args: unknown[]) =>
			Object.assign(fn(...args), {
				refresh: async () => {
					h.queryRefreshes++;
				}
			});
		return Object.assign(wrapped, { __: { type: 'query' } });
	},
	getRequestEvent: () => ({ locals: { user: { id: 1, login: 'octo' } }, cookies: {} })
}));
vi.mock('#lib/server/repo-access.ts', () => ({
	getRepositoryAccess: () => ({
		get configured() {
			return h.configured;
		},
		list: async (login: string, opts: { refresh?: boolean } = {}) => {
			h.listCalls.push({ login, refresh: !!opts.refresh });
			if (h.listFails) throw new Error('github down');
			return { repositories: h.listed, fetchedAt: new Date('2026-10-16T12:00:00Z') };
		},
		check: async (login: string, key: string) => {
			h.checks.push([login, key]);
			return h.allowed.has(key) ? repo(key.replace(/^github:/, '')) : null;
		}
	})
}));
vi.mock('#lib/server/kube.ts', async (orig) => ({
	...(await orig<object>()),
	getProjectStore: () => ({
		list: async () => h.projects,
		get: async (n: string) => h.projects.find((p) => p.metadata.name === n) ?? null,
		create: async (p: Project) => (h.projects.push(p), p),
		update: async (n: string, patch: unknown) => (h.updates.push([n, patch]), h.projects[0])
	})
}));
vi.mock('#lib/server/db.ts', () => ({ getSql: () => ({}) }));
vi.mock('#lib/server/work-service.ts', () => ({
	workService: () => ({ cancelForRemovedRepos: async () => 0 })
}));

function repo(full: string): UserRepository {
	return {
		repo: `github:${full}`,
		installationId: 1,
		accountLogin: full.split('/')[0],
		private: false,
		defaultBranch: 'main'
	};
}
const project = (name: string, urls: string[]): Project => ({
	apiVersion: 'enzarb.dev/v1alpha1',
	kind: 'Project',
	metadata: { name },
	spec: {
		owner: 'octo',
		displayName: name,
		repositories: urls.map((url) => ({ url, connector: 'github', defaultBranch: 'main' })),
		runMode: 'worktree',
		triggers: { issues: true, changeRequests: true, mentions: true }
	}
});

const remote = await import('./projects.remote.ts');
type Fn = (arg?: unknown) => Promise<unknown>;
const call = (f: unknown, arg?: unknown) => (f as Fn)(arg);

beforeEach(() => {
	h.configured = true;
	h.listed = [repo('acme/web')];
	h.listFails = false;
	h.listCalls = [];
	h.allowed = new Set(['github:acme/web']);
	h.checks = [];
	h.queryRefreshes = 0;
	h.projects = [];
	h.updates = [];
});

describe('repositoryChoices', () => {
	it('serves the live list (display cache allowed)', async () => {
		const out = (await call(remote.repositoryChoices)) as {
			repositories: unknown[];
			problem: unknown;
		};
		expect(out.repositories).toHaveLength(1);
		expect(out.problem).toBeNull();
		expect(h.listCalls).toEqual([{ login: 'octo', refresh: false }]);
	});

	it('turns a GitHub failure into a problem, not an error', async () => {
		vi.spyOn(console, 'error').mockImplementation(() => {});
		h.listFails = true;
		const out = (await call(remote.repositoryChoices)) as {
			repositories: unknown[];
			problem: { code: string };
		};
		expect(out.repositories).toEqual([]);
		expect(out.problem.code).toBe('unavailable');
	});
});

describe('refreshRepositoryChoices', () => {
	it('bypasses the display cache and refreshes the query', async () => {
		await call(remote.refreshRepositoryChoices);
		expect(h.listCalls[0]).toEqual({ login: 'octo', refresh: true });
		expect(h.queryRefreshes).toBe(1);
	});
});

describe('createProjects', () => {
	const input = (repositories: string[]) => ({
		plans: [{ name: 'web', repositories }],
		triggers: { issues: true, changeRequests: true, mentions: true }
	});

	it('authorizes each repository with a live check, not the list', async () => {
		h.listed = []; // the cached list is irrelevant to authorization
		const out = (await call(remote.createProjects, input(['github:acme/web']))) as {
			ok: boolean;
			results: { status: string }[];
		};
		expect(out.results[0].status).toBe('created');
		expect(h.checks).toEqual([['octo', 'github:acme/web']]);
		expect(h.listCalls).toEqual([]);
	});

	it('refuses a repository whose access was revoked even if the list still shows it', async () => {
		h.allowed.clear();
		const out = (await call(remote.createProjects, input(['github:acme/web']))) as {
			results: { status: string; reason?: string }[];
		};
		expect(out.results[0]).toMatchObject({
			status: 'failed',
			reason: 'acme/web is not available to you'
		});
		expect(h.projects).toEqual([]);
	});

	it('is unavailable without GitHub App credentials', async () => {
		h.configured = false;
		const out = await call(remote.createProjects, input(['github:acme/web']));
		expect(out).toMatchObject({ ok: false, code: 'unavailable' });
		expect(h.checks).toEqual([]);
	});
});

describe('setRepositories', () => {
	it('checks only added repositories live', async () => {
		h.projects = [project('p', ['https://github.com/acme/old'])];
		const out = await call(remote.setRepositories, {
			name: 'p',
			repositories: ['github:acme/old', 'github:acme/web']
		});
		expect(out).toMatchObject({ ok: true });
		expect(h.checks).toEqual([['octo', 'github:acme/web']]);
	});

	it('refuses an added repository the user lost write access to', async () => {
		h.projects = [project('p', ['https://github.com/acme/old'])];
		h.allowed.clear();
		const out = await call(remote.setRepositories, {
			name: 'p',
			repositories: ['github:acme/old', 'github:acme/web']
		});
		expect(out).toMatchObject({ ok: false, code: 'invalid' });
		expect(h.updates).toEqual([]);
	});
});

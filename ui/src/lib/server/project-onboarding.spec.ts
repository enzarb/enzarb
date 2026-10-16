import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { KubeError, type ProjectStore } from './kube.ts';
import {
	createProjects,
	repositoryChoices,
	setDisplayName,
	setRepositories,
	listProblem
} from './project-onboarding.ts';
import type { UserRepositoryList } from './user-repositories.ts';

const NOW = new Date('2026-06-10T12:00:00Z');
const triggers = { issues: true, changeRequests: false, mentions: true };

function repo(full: string, extra: Partial<{ private: boolean; defaultBranch: string }> = {}) {
	return {
		repo: `github:${full}`,
		installationId: 1,
		accountLogin: full.split('/')[0],
		private: false,
		defaultBranch: 'main',
		...extra
	};
}
const snap = (
	repositories = [repo('acme/web'), repo('acme/api', { defaultBranch: 'trunk' }), repo('me/own')]
): UserRepositoryList => ({ repositories, fetchedAt: NOW });
/** A live check answering from `list` (what GitHub would say right now), recording each call. */
const checkOf = (list = snap()) =>
	vi.fn(async (key: string) => list.repositories.find((r) => r.repo === key) ?? null);

function project(name: string, owner: string, urls: string[]): Project {
	return {
		apiVersion: 'enzarb.dev/v1alpha1',
		kind: 'Project',
		metadata: { name },
		spec: {
			owner,
			displayName: name,
			repositories: urls.map((url) => ({ url, connector: 'github', defaultBranch: 'main' })),
			runMode: 'worktree',
			triggers: { issues: true, changeRequests: true, mentions: true }
		}
	};
}

function fakeStore(initial: Project[] = []) {
	const items = new Map(initial.map((p) => [p.metadata.name, p]));
	const realCreate = async (p: Project) => {
		if (items.has(p.metadata.name)) throw new KubeError('conflict', 'exists');
		items.set(p.metadata.name, p);
		return p;
	};
	const store: ProjectStore & { items: Map<string, Project>; realCreate: typeof realCreate } = {
		items,
		realCreate,
		list: vi.fn(async () => [...items.values()]),
		get: vi.fn(async (n) => items.get(n) ?? null),
		create: vi.fn(realCreate),
		update: vi.fn(async (n) => items.get(n)!),
		delete: vi.fn(async () => {})
	};
	return store;
}

const run = (
	store: ProjectStore,
	plans: { name: string; repositories: string[]; maxRuns?: number }[],
	check = checkOf()
) => createProjects({ store, owner: 'alice', check }, plans, triggers);

describe('listProblem', () => {
	it('accepts a non-empty list', () => expect(listProblem(snap())).toBeNull());
	it('explains an empty list', () => {
		expect(listProblem(snap([]))?.message).toMatch(
			/repositories the enzarb GitHub App can access where you have write access/
		);
	});
});

describe('createProjects: authorization', () => {
	let store: ReturnType<typeof fakeStore>;
	beforeEach(() => (store = fakeStore()));

	it('checks every repository live at add time and refuses revoked access', async () => {
		const check = checkOf(snap([repo('acme/api', { defaultBranch: 'trunk' })]));
		const out = await run(
			store,
			[
				{ name: 'web', repositories: ['github:acme/web'] },
				{ name: 'api', repositories: ['github:acme/api'] }
			],
			check
		);
		expect(check.mock.calls.map((c) => c[0])).toEqual(['github:acme/web', 'github:acme/api']);
		expect(out.ok && out.results.map((r) => r.status)).toEqual(['failed', 'created']);
		expect([...store.items.keys()]).toEqual(['api']);
	});

	it('fails a plan holding a repository the check refuses, others still succeed', async () => {
		const out = await run(store, [
			{ name: 'evil', repositories: ['github:victim/secret'] },
			{ name: 'mixed', repositories: ['github:acme/web', 'github:victim/secret'] },
			{ name: 'good', repositories: ['github:acme/web'] }
		]);
		expect(out).toMatchObject({ ok: true });
		const results = out.ok ? out.results : [];
		expect(results.map((r) => r.status)).toEqual(['failed', 'failed', 'created']);
		expect(results[0]).toMatchObject({ reason: 'victim/secret is not available to you' });
		expect([...store.items.keys()]).toEqual(['good']);
	});

	it('builds url, connector and branch from the live check, never from the request', async () => {
		await run(store, [{ name: 'both', repositories: ['github:acme/web', 'github:acme/api'] }]);
		const p = store.items.get('both')!;
		expect(p.spec.owner).toBe('alice');
		expect(p.spec.repositories).toEqual([
			{ url: 'https://github.com/acme/web', connector: 'github', defaultBranch: 'main' },
			{ url: 'https://github.com/acme/api', connector: 'github', defaultBranch: 'trunk' }
		]);
		expect(p.spec.triggers).toEqual(triggers);
	});

	it('writes spec.maxRuns only when the plan sets it', async () => {
		await run(store, [
			{ name: 'plain', repositories: ['github:acme/web'] },
			{ name: 'busy', repositories: ['github:acme/api'], maxRuns: 8 }
		]);
		expect('maxRuns' in store.items.get('plain')!.spec).toBe(false);
		expect(store.items.get('busy')!.spec.maxRuns).toBe(8);
	});

	it('rejects URL-shaped or case-mismatched keys (only exact keys match)', async () => {
		const out = await run(store, [
			{ name: 'a', repositories: ['https://github.com/acme/web'] },
			{ name: 'b', repositories: ['github:Acme/Web'] }
		]);
		expect(out.ok && out.results.map((r) => r.status)).toEqual(['failed', 'failed']);
		expect(store.create).not.toHaveBeenCalled();
	});
});

describe('createProjects: duplicates and validation', () => {
	it('refuses a repository already in a project (own: names it; other owner: does not)', async () => {
		const store = fakeStore([
			project('mine', 'alice', ['https://github.com/acme/web']),
			project('theirs', 'bob', ['https://github.com/acme/api'])
		]);
		const out = await run(store, [
			{ name: 'one', repositories: ['github:acme/web'] },
			{ name: 'two', repositories: ['github:acme/api'] }
		]);
		const r = out.ok ? out.results : [];
		expect(r[0]).toMatchObject({ status: 'failed', reason: 'acme/web is already in "mine"' });
		expect(r[1]).toMatchObject({
			status: 'failed',
			reason: 'acme/api is already in another project'
		});
		expect(r[1].status === 'failed' && r[1].reason).not.toContain('theirs');
	});

	it('refuses a repository used twice in the request: the first wins', async () => {
		const store = fakeStore();
		const out = await run(store, [
			{ name: 'first', repositories: ['github:acme/web'] },
			{ name: 'second', repositories: ['github:acme/web', 'github:me/own'] }
		]);
		const r = out.ok ? out.results : [];
		expect(r.map((x) => x.status)).toEqual(['created', 'failed']);
		expect(r[1]).toMatchObject({ reason: 'acme/web is already in "first"' });
		expect([...store.items.keys()]).toEqual(['first']);
	});

	it('refuses a repository listed twice inside one plan', async () => {
		const out = await run(fakeStore(), [
			{ name: 'x', repositories: ['github:acme/web', 'github:acme/web'] }
		]);
		expect(out.ok && out.results[0]).toMatchObject({ status: 'failed' });
	});

	it('refuses an existing project name (any owner) and a repeated name in the request', async () => {
		const store = fakeStore([project('taken', 'bob', [])]);
		const out = await run(store, [
			{ name: 'taken', repositories: ['github:acme/web'] },
			{ name: 'new', repositories: ['github:acme/web'] },
			{ name: 'new', repositories: ['github:acme/api'] }
		]);
		const r = out.ok ? out.results : [];
		expect(r.map((x) => x.status)).toEqual(['failed', 'created', 'failed']);
		expect(r[0]).toMatchObject({ reason: 'A project named "taken" already exists' });
		expect(r[2]).toMatchObject({ reason: 'A project named "new" already exists' });
	});

	it('re-checks the cluster before every create (a repo taken mid-request is refused)', async () => {
		const store = fakeStore();
		const create = store.create as ReturnType<typeof vi.fn>;
		const real = store.realCreate;
		create.mockImplementationOnce(async (p: Project) => {
			// Someone else grabs acme/api while the first create is in flight.
			store.items.set('sneaky', project('sneaky', 'bob', ['https://github.com/acme/api']));
			return real(p);
		});
		const out = await run(store, [
			{ name: 'a', repositories: ['github:acme/web'] },
			{ name: 'b', repositories: ['github:acme/api'] }
		]);
		expect(out.ok && out.results.map((r) => r.status)).toEqual(['created', 'failed']);
		expect(store.list).toHaveBeenCalledTimes(2);
	});

	it.each([['Bad Name'], [''], ['a'.repeat(55)], ['-x']])(
		'refuses invalid name %j',
		async (name) => {
			const store = fakeStore();
			const out = await run(store, [{ name, repositories: ['github:acme/web'] }]);
			expect(out.ok && out.results[0].status).toBe('failed');
			expect(store.create).not.toHaveBeenCalled();
		}
	);

	it('trims the display name and truncates echoed keys', async () => {
		const store = fakeStore();
		const out = await createProjects(
			{ store, owner: 'alice', check: checkOf() },
			[
				{ name: 'dn', displayName: '  Nice  ', repositories: ['github:acme/web'] },
				{ name: 'long', repositories: [`github:${'x'.repeat(500)}/y`] }
			],
			triggers
		);
		expect(store.items.get('dn')?.spec.displayName).toBe('Nice');
		const long = out.ok && out.results[1];
		expect(long && long.status === 'failed' && long.reason.length).toBeLessThan(140);
	});

	it('creates the project under the trimmed name', async () => {
		const store = fakeStore();
		const out = await run(store, [{ name: '  padded  ', repositories: ['github:acme/web'] }]);
		expect(out.ok && out.results[0]).toMatchObject({ name: 'padded', status: 'created' });
		expect(store.items.get('padded')?.metadata.name).toBe('padded');
	});

	it('refuses a plan without repositories', async () => {
		const out = await run(fakeStore(), [{ name: 'empty', repositories: [] }]);
		expect(out.ok && out.results[0]).toMatchObject({ status: 'failed' });
	});
});

describe('createProjects: existing projects with non-canonical urls', () => {
	it.each([
		'https://www.github.com/acme/web',
		'https://github.com/acme/web/',
		'https://github.com/acme/web.git',
		'https://GitHub.com/Acme/Web.git/'
	])('sees %s as the same repository', async (url) => {
		const store = fakeStore([project('legacy', 'alice', [url])]);
		const out = await run(store, [{ name: 'new', repositories: ['github:acme/web'] }]);
		expect(out.ok && out.results[0]).toMatchObject({
			status: 'failed',
			reason: 'acme/web is already in "legacy"'
		});
	});
});

describe('createProjects: concurrency', () => {
	it('serializes list+create so concurrent requests cannot claim one repository', async () => {
		const store = fakeStore();
		const real = store.realCreate;
		(store.create as ReturnType<typeof vi.fn>).mockImplementation(async (p: Project) => {
			await new Promise((r) => setTimeout(r, 20)); // a slow API server
			return real(p);
		});
		const plan = (name: string) => [{ name, repositories: ['github:acme/web'] }];
		const [a, b] = await Promise.all([run(store, plan('one')), run(store, plan('two'))]);
		const statuses = [a, b].map((o) => (o.ok ? o.results[0].status : 'x')).sort();
		expect(statuses).toEqual(['created', 'failed']);
		expect(store.items.size).toBe(1);
	});
	it('keeps going after a failed step', async () => {
		const store = fakeStore();
		(store.list as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('down'));
		vi.spyOn(console, 'error').mockImplementation(() => {});
		const out = await run(store, [
			{ name: 'a', repositories: ['github:acme/web'] },
			{ name: 'b', repositories: ['github:acme/api'] }
		]);
		expect(out.ok && out.results.map((r) => r.status)).toEqual(['failed', 'created']);
	});
});

describe('createProjects: result collection', () => {
	it('keeps going after a cluster failure and reports each project', async () => {
		const store = fakeStore();
		const create = store.create as ReturnType<typeof vi.fn>;
		const real = store.realCreate;
		vi.spyOn(console, 'error').mockImplementation(() => {});
		create.mockImplementation(async (p: Project) => {
			if (p.metadata.name === 'boom') throw new KubeError('other', 'Kubernetes API error 500');
			return real(p);
		});
		const out = await run(store, [
			{ name: 'web', repositories: ['github:acme/web'] },
			{ name: 'boom', repositories: ['github:acme/api'] },
			{ name: 'own', repositories: ['github:me/own'] }
		]);
		expect(out).toEqual({
			ok: true,
			results: [
				{ name: 'web', status: 'created', repositories: ['github:acme/web'] },
				{
					name: 'boom',
					status: 'failed',
					repositories: ['github:acme/api'],
					reason: 'The cluster refused or could not create the project'
				},
				{ name: 'own', status: 'created', repositories: ['github:me/own'] }
			]
		});
	});

	it('maps a create-time name conflict to a clear failure', async () => {
		const store = fakeStore();
		// list() misses the project (race), create() reports the conflict
		(store.list as ReturnType<typeof vi.fn>).mockResolvedValue([]);
		store.items.set('web', project('web', 'bob', []));
		const out = await run(store, [{ name: 'web', repositories: ['github:acme/web'] }]);
		expect(out.ok && out.results[0]).toMatchObject({
			status: 'failed',
			reason: 'A project named "web" already exists'
		});
	});
});

describe('repositoryChoices', () => {
	it('lists the live list and marks only the caller’s projects as holders', () => {
		const choices = repositoryChoices(
			snap(),
			[
				project('mine', 'alice', ['https://github.com/acme/web']),
				project('theirs', 'bob', ['https://github.com/acme/api'])
			],
			'alice'
		);
		expect(choices.repositories.map((r) => r.repo)).toEqual([
			'github:acme/web',
			'github:acme/api',
			'github:me/own'
		]);
		expect(choices.taken).toEqual({ 'github:acme/web': 'mine' });
		expect(choices.projectNames).toEqual(['mine']);
		expect(choices.problem).toBeNull();
	});
	it('reports an empty list and passes the fetch time', () => {
		const choices = repositoryChoices(snap([]), [], 'alice');
		expect(choices.problem?.code).toBe('empty');
		expect(choices.fetchedAt).toBe(NOW.toISOString());
	});
});

describe('setDisplayName', () => {
	const rename = (store: ProjectStore, input: string, owner = 'alice', name = 'p') =>
		setDisplayName({ store, owner }, name, input);

	it('trims and patches only displayName', async () => {
		const store = fakeStore([project('p', 'alice', [])]);
		expect(await rename(store, '  New name  ')).toEqual({ ok: true, displayName: 'New name' });
		expect(store.update).toHaveBeenCalledWith('p', { displayName: 'New name' });
	});

	it('rejects empty, whitespace-only and over-long names; accepts exactly 100', async () => {
		const store = fakeStore([project('p', 'alice', [])]);
		expect(await rename(store, '')).toMatchObject({ ok: false, code: 'invalid' });
		expect(await rename(store, '   ')).toMatchObject({ ok: false, code: 'invalid' });
		expect(await rename(store, 'x'.repeat(101))).toMatchObject({ ok: false, code: 'invalid' });
		expect(store.update).not.toHaveBeenCalled();
		expect((await rename(store, 'x'.repeat(100))).ok).toBe(true);
	});

	it('returns not-found for non-owners and unknown projects without updating', async () => {
		const store = fakeStore([project('p', 'alice', [])]);
		expect(await rename(store, 'N', 'mallory')).toMatchObject({ ok: false, code: 'not-found' });
		expect(await rename(store, 'N', 'alice', 'nope')).toMatchObject({
			ok: false,
			code: 'not-found'
		});
		expect(store.update).not.toHaveBeenCalled();
	});
});

describe('setRepositories', () => {
	const setRepos = (store: ProjectStore, keys: string[], check = checkOf(), owner = 'alice') =>
		setRepositories({ store, owner, check }, 'p', keys);
	const old = 'https://github.com/acme/archived';

	it('replaces the list: keeps stored entries, builds added ones from the live check', async () => {
		const store = fakeStore([project('p', 'alice', [old, 'https://github.com/acme/web'])]);
		const r = await setRepos(store, ['github:acme/api', 'github:acme/web']);
		expect(r.ok).toBe(true);
		expect(store.update).toHaveBeenCalledWith('p', {
			repositories: [
				{ url: 'https://github.com/acme/api', connector: 'github', defaultBranch: 'trunk' },
				{ url: 'https://github.com/acme/web', connector: 'github', defaultBranch: 'main' }
			]
		});
	});

	it('keeps and removes stored repositories without asking GitHub', async () => {
		const store = fakeStore([project('p', 'alice', [old, 'https://github.com/acme/web'])]);
		const check = checkOf(snap([]));
		const r = await setRepos(store, ['github:acme/web'], check);
		expect(r.ok).toBe(true);
		expect(check).not.toHaveBeenCalled();
	});

	it('checks an added repository live: revoked access is refused', async () => {
		const store = fakeStore([project('p', 'alice', [old])]);
		const check = checkOf(snap([]));
		const r = await setRepos(store, [`github:acme/archived`, 'github:acme/web'], check);
		expect(r).toMatchObject({ ok: false, message: 'acme/web is not available to you' });
		expect(check.mock.calls.map((c) => c[0])).toEqual(['github:acme/web']);
		expect(store.update).not.toHaveBeenCalled();
	});

	it('refuses repositories the check refuses, empty lists and duplicates', async () => {
		const store = fakeStore([project('p', 'alice', [old])]);
		expect(await setRepos(store, ['github:evil/repo'])).toMatchObject({
			ok: false,
			message: 'evil/repo is not available to you'
		});
		expect(await setRepos(store, [])).toMatchObject({ ok: false, code: 'invalid' });
		expect(await setRepos(store, ['github:acme/web', 'github:acme/web'])).toMatchObject({
			ok: false,
			code: 'invalid'
		});
		expect(store.update).not.toHaveBeenCalled();
	});

	it('enforces one Project per repository', async () => {
		const store = fakeStore([
			project('p', 'alice', [old]),
			project('mine', 'alice', ['https://github.com/acme/web']),
			project('theirs', 'bob', ['https://github.com/acme/api'])
		]);
		expect(await setRepos(store, ['github:acme/web'])).toMatchObject({
			message: 'acme/web is already in "mine"'
		});
		expect(await setRepos(store, ['github:acme/api'])).toMatchObject({
			message: 'acme/api is already in another project'
		});
		expect(store.update).not.toHaveBeenCalled();
	});

	it('is not-found for other owners', async () => {
		const store = fakeStore([project('p', 'bob', [old])]);
		expect(await setRepos(store, ['github:acme/web'])).toMatchObject({ code: 'not-found' });
	});

	describe('work items of removed repositories', () => {
		const withWork = (
			store: ProjectStore,
			keys: string[],
			work: { cancelForRemovedRepos: (p: string, r: string[]) => Promise<unknown> }
		) => setRepositories({ store, owner: 'alice', check: checkOf(), work }, 'p', keys);

		it('cancels only for the repositories that left the Project, after the update', async () => {
			const store = fakeStore([
				project('p', 'alice', [old, 'https://github.com/acme/web', 'https://github.com/acme/api'])
			]);
			const cancel = vi.fn(async () => 0);
			const r = await withWork(store, ['github:acme/web', 'github:me/own'], {
				cancelForRemovedRepos: cancel
			});
			expect(r.ok).toBe(true);
			expect(cancel).toHaveBeenCalledOnce();
			expect(cancel).toHaveBeenCalledWith('p', ['github:acme/archived', 'github:acme/api']);
			expect(vi.mocked(store.update).mock.invocationCallOrder[0]).toBeLessThan(
				cancel.mock.invocationCallOrder[0]
			);
		});

		it('does not cancel when repositories are only added or kept', async () => {
			const store = fakeStore([project('p', 'alice', ['https://github.com/acme/web'])]);
			const cancel = vi.fn(async () => 0);
			const r = await withWork(store, ['github:acme/web', 'github:acme/api'], {
				cancelForRemovedRepos: cancel
			});
			expect(r.ok).toBe(true);
			expect(cancel).not.toHaveBeenCalled();
		});

		it('does not cancel when the update is refused', async () => {
			const store = fakeStore([project('p', 'alice', [old])]);
			const cancel = vi.fn(async () => 0);
			const r = await withWork(store, ['github:evil/repo'], { cancelForRemovedRepos: cancel });
			expect(r.ok).toBe(false);
			expect(cancel).not.toHaveBeenCalled();
		});

		it('still succeeds when cancelling fails', async () => {
			const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
			const store = fakeStore([project('p', 'alice', [old, 'https://github.com/acme/web'])]);
			const r = await withWork(store, ['github:acme/web'], {
				cancelForRemovedRepos: async () => {
					throw new Error('db down');
				}
			});
			expect(r).toMatchObject({ ok: true, repositories: ['github:acme/web'] });
			expect(store.update).toHaveBeenCalledOnce();
			expect(warn).toHaveBeenCalled();
			warn.mockRestore();
		});
	});
});

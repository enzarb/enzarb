import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { KubeError, type ProjectStore } from './kube.ts';
import { createProjects, repositoryChoices, snapshotProblem } from './project-onboarding.ts';
import type { UserRepositorySnapshot } from './user-repositories.ts';

const NOW = new Date('2026-06-10T12:00:00Z');
const hoursAgo = (h: number) => new Date(NOW.getTime() - h * 3600_000);
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
	fetchedAt: Date | null = hoursAgo(1),
	repositories = [repo('acme/web'), repo('acme/api', { defaultBranch: 'trunk' }), repo('me/own')]
): UserRepositorySnapshot => ({ repositories, fetchedAt });

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
		delete: vi.fn(async () => {})
	};
	return store;
}

const run = (
	store: ProjectStore,
	plans: { name: string; repositories: string[] }[],
	snapshot = snap()
) => createProjects({ store, owner: 'alice', snapshot, now: NOW }, plans, triggers);

describe('snapshotProblem', () => {
	it('accepts a fresh snapshot', () => expect(snapshotProblem(snap(), NOW)).toBeNull());
	it('accepts up to 24 hours and rejects beyond', () => {
		expect(snapshotProblem(snap(hoursAgo(24)), NOW)).toBeNull();
		expect(snapshotProblem(snap(hoursAgo(24.01)), NOW)?.code).toBe('stale-snapshot');
	});
	it('rejects an empty snapshot', () => {
		expect(snapshotProblem(snap(null, []), NOW)?.code).toBe('no-snapshot');
	});
});

describe('createProjects: authorization', () => {
	let store: ReturnType<typeof fakeStore>;
	beforeEach(() => (store = fakeStore()));

	it('refuses everything when the snapshot is stale, creating nothing', async () => {
		const out = await run(
			store,
			[{ name: 'web', repositories: ['github:acme/web'] }],
			snap(hoursAgo(30))
		);
		expect(out).toMatchObject({ ok: false, code: 'stale-snapshot' });
		expect(store.create).not.toHaveBeenCalled();
	});

	it('refuses everything without a snapshot', async () => {
		const out = await run(
			store,
			[{ name: 'web', repositories: ['github:acme/web'] }],
			snap(null, [])
		);
		expect(out).toMatchObject({ ok: false, code: 'no-snapshot' });
		expect(store.create).not.toHaveBeenCalled();
	});

	it('fails a plan holding a repository outside the snapshot, others still succeed', async () => {
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

	it('builds url, connector and branch from the snapshot, never from the request', async () => {
		await run(store, [{ name: 'both', repositories: ['github:acme/web', 'github:acme/api'] }]);
		const p = store.items.get('both')!;
		expect(p.spec.owner).toBe('alice');
		expect(p.spec.repositories).toEqual([
			{ url: 'https://github.com/acme/web', connector: 'github', defaultBranch: 'main' },
			{ url: 'https://github.com/acme/api', connector: 'github', defaultBranch: 'trunk' }
		]);
		expect(p.spec.triggers).toEqual(triggers);
	});

	it('rejects URL-shaped or case-mismatched keys (only exact snapshot keys match)', async () => {
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
			{ store, owner: 'alice', snapshot: snap(), now: NOW },
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
	it('lists the snapshot and marks only the caller’s projects as holders', () => {
		const choices = repositoryChoices(
			snap(),
			[
				project('mine', 'alice', ['https://github.com/acme/web']),
				project('theirs', 'bob', ['https://github.com/acme/api'])
			],
			'alice',
			NOW
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
	it('reports a stale snapshot', () => {
		expect(repositoryChoices(snap(hoursAgo(48)), [], 'alice', NOW).problem?.code).toBe(
			'stale-snapshot'
		);
	});
});

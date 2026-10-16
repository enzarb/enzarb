import { generateKeyPairSync } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import { createRepositoryAccess, groupByAccount, installUrl } from './user-repositories.ts';

const API = 'https://api.github.com';
const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const pem = privateKey.export({ type: 'pkcs1', format: 'pem' }) as string;
const NOW = Date.parse('2026-10-16T12:00:00Z');

const json = (body: unknown, opts: { status?: number; link?: string } = {}) =>
	new Response(JSON.stringify(body), {
		status: opts.status ?? 200,
		headers: { 'content-type': 'application/json', ...(opts.link ? { link: opts.link } : {}) }
	});
const repo = (full_name: string, priv = false, extra: object = {}) => ({
	full_name,
	private: priv,
	default_branch: 'main',
	...extra
});

/**
 * A fake GitHub App API. `installations` maps installation id -> account and repository pages;
 * `roles` maps `owner/repo` -> the user's role (missing = 404).
 */
function fakeGitHub(world: {
	installations: {
		id: number;
		login: string;
		pages: ReturnType<typeof repo>[][];
		suspended?: boolean;
	}[];
	roles: Record<string, string>;
	failPermission?: boolean;
}) {
	const calls: { url: string; auth: string }[] = [];
	const fn = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
		const url = String(input);
		const auth = (init?.headers as Record<string, string>).Authorization;
		calls.push({ url, auth });
		if (url === `${API}/app/installations?per_page=100`)
			return json(
				world.installations.map((i) => ({
					id: i.id,
					account: { login: i.login },
					suspended_at: i.suspended ? '2026-01-01T00:00:00Z' : null
				}))
			);
		let m = /\/app\/installations\/(\d+)\/access_tokens$/.exec(url);
		if (m && init?.method === 'POST')
			return json(
				{ token: `inst-${m[1]}`, expires_at: new Date(NOW + 3600_000).toISOString() },
				{ status: 201 }
			);
		m = /\/installation\/repositories\?per_page=100(?:&page=(\d+))?$/.exec(url);
		if (m) {
			const inst = world.installations.find((i) => auth === `Bearer inst-${i.id}`)!;
			const page = Number(m[1] ?? 1);
			const next =
				page < inst.pages.length
					? `<${API}/installation/repositories?per_page=100&page=${page + 1}>; rel="next"`
					: undefined;
			return json({ repositories: inst.pages[page - 1] ?? [] }, { link: next });
		}
		m = /\/repos\/([^/]+)\/([^/]+)\/collaborators\/([^/]+)\/permission$/.exec(url);
		if (m) {
			if (world.failPermission) return json({}, { status: 500 });
			const role = world.roles[`${m[1]}/${m[2]}`.toLowerCase()];
			return role
				? json({ permission: role === 'maintain' ? 'write' : role, role_name: role })
				: json({}, { status: 404 });
		}
		m = /\/repos\/([^/]+)\/([^/]+)\/installation$/.exec(url);
		if (m) {
			const inst = world.installations.find((i) =>
				i.pages.flat().some((r) => r.full_name.toLowerCase() === `${m![1]}/${m![2]}`.toLowerCase())
			);
			return inst
				? json({
						id: inst.id,
						account: { login: inst.login },
						suspended_at: inst.suspended ? 'x' : null
					})
				: json({}, { status: 404 });
		}
		m = /\/repos\/([^/]+)\/([^/]+)$/.exec(url);
		if (m) {
			const r = world.installations
				.flatMap((i) => i.pages.flat())
				.find((r) => r.full_name.toLowerCase() === `${m![1]}/${m![2]}`.toLowerCase());
			return r ? json(r) : json({}, { status: 404 });
		}
		return new Response('unexpected ' + url, { status: 500 });
	}) as unknown as typeof fetch;
	return { fn, calls };
}

function setup(world: Parameters<typeof fakeGitHub>[0], now = () => NOW) {
	const gh = fakeGitHub(world);
	const access = createRepositoryAccess({ appId: '1', privateKey: pem, fetch: gh.fn, now });
	return { gh, access };
}

const twoOrgs = (): Parameters<typeof fakeGitHub>[0] => ({
	installations: [
		{
			id: 1,
			login: 'Acme',
			pages: [[repo('Acme/Web', true), repo('Acme/Docs')], [repo('Acme/New')]]
		},
		{
			id: 2,
			login: 'octo',
			pages: [[repo('octo/own'), repo('octo/old', false, { archived: true })]]
		}
	],
	roles: {
		'acme/web': 'write',
		'acme/docs': 'read',
		'acme/new': 'maintain',
		'octo/own': 'admin',
		'octo/old': 'admin'
	} as Record<string, string>
});

describe('createRepositoryAccess.list', () => {
	it('walks every installation and page, keeps write/maintain/admin, sorted and lowercased', async () => {
		const { access, gh } = setup(twoOrgs());
		const { repositories, fetchedAt } = await access.list('me');
		expect(repositories).toEqual([
			{
				repo: 'github:acme/new',
				installationId: 1,
				accountLogin: 'Acme',
				private: false,
				defaultBranch: 'main'
			},
			{
				repo: 'github:acme/web',
				installationId: 1,
				accountLogin: 'Acme',
				private: true,
				defaultBranch: 'main'
			},
			{
				repo: 'github:octo/own',
				installationId: 2,
				accountLogin: 'octo',
				private: false,
				defaultBranch: 'main'
			}
		]);
		expect(fetchedAt.getTime()).toBe(NOW);
		// Each installation is listed with its own token; permission checks use the user's login.
		expect(
			gh.calls.filter((c) => c.url.includes('/installation/repositories')).map((c) => c.auth)
		).toEqual(['Bearer inst-1', 'Bearer inst-1', 'Bearer inst-2']);
		expect(
			gh.calls.some((c) => c.url.endsWith('/repos/Acme/Docs/collaborators/me/permission'))
		).toBe(true);
		// Archived repositories are not offered (nor checked).
		expect(gh.calls.some((c) => c.url.includes('/octo/old/'))).toBe(false);
	});

	it('skips suspended installations', async () => {
		const world = twoOrgs();
		world.installations[0].suspended = true;
		const { access } = setup(world);
		expect((await access.list('me')).repositories.map((r) => r.repo)).toEqual(['github:octo/own']);
	});

	it('serves the display cache for a minute, dedupes concurrent reads, and Refresh bypasses it', async () => {
		let t = NOW;
		const world = twoOrgs();
		const { access, gh } = setup(world, () => t);
		const count = () =>
			gh.calls.filter((c) => c.url.startsWith(`${API}/app/installations?`)).length;
		await Promise.all([access.list('me'), access.list('ME')]);
		expect(count()).toBe(1);
		// A repository created on GitHub meanwhile: hidden by the cache, shown by a refresh.
		world.installations[1].pages[0].push(repo('octo/fresh'));
		world.roles['octo/fresh'] = 'write';
		t += 30_000;
		expect((await access.list('me')).repositories.map((r) => r.repo)).not.toContain(
			'github:octo/fresh'
		);
		expect(count()).toBe(1);
		expect((await access.list('me', { refresh: true })).repositories.map((r) => r.repo)).toContain(
			'github:octo/fresh'
		);
		expect(count()).toBe(2);
		t += 61_000;
		await access.list('me');
		expect(count()).toBe(3);
	});

	it('caches per user', async () => {
		const { access, gh } = setup(twoOrgs());
		await access.list('me');
		await access.list('you');
		expect(gh.calls.filter((c) => c.url.startsWith(`${API}/app/installations?`))).toHaveLength(2);
	});

	it('throws on a GitHub failure and does not cache it', async () => {
		const world = { ...twoOrgs(), failPermission: true };
		const { access } = setup(world);
		await expect(access.list('me')).rejects.toThrow(/500/);
		world.failPermission = false;
		expect((await access.list('me')).repositories).toHaveLength(3);
	});

	it('is empty without App credentials and never calls GitHub', async () => {
		const f = vi.fn();
		const access = createRepositoryAccess({ fetch: f as unknown as typeof fetch });
		expect(access.configured).toBe(false);
		expect((await access.list('me')).repositories).toEqual([]);
		expect(await access.check('me', 'github:acme/web')).toBeNull();
		expect(f).not.toHaveBeenCalled();
	});
});

describe('createRepositoryAccess.check (live, at add time)', () => {
	it('returns the repository from GitHub when the user has write access', async () => {
		const { access } = setup(twoOrgs());
		expect(await access.check('me', 'github:acme/web')).toEqual({
			repo: 'github:acme/web',
			installationId: 1,
			accountLogin: 'Acme',
			private: true,
			defaultBranch: 'main'
		});
	});

	it('refuses read-only, unknown, archived, uninstalled and malformed keys', async () => {
		const { access } = setup(twoOrgs());
		expect(await access.check('me', 'github:acme/docs')).toBeNull();
		expect(await access.check('me', 'github:evil/repo')).toBeNull();
		expect(await access.check('me', 'github:octo/old')).toBeNull();
		expect(await access.check('me', 'https://github.com/acme/web')).toBeNull();
		expect(await access.check('me', 'github:acme/../x')).toBeNull();
	});

	it('is never served from the display cache: revoked access is refused immediately', async () => {
		const world = twoOrgs();
		const { access } = setup(world);
		expect((await access.list('me')).repositories.map((r) => r.repo)).toContain('github:acme/web');
		delete world.roles['acme/web'];
		expect(await access.check('me', 'github:acme/web')).toBeNull();
	});

	it('sees a repository created after the list was cached', async () => {
		const world = twoOrgs();
		const { access } = setup(world);
		await access.list('me');
		world.installations[0].pages[1].push(repo('Acme/Brand-New'));
		world.roles['acme/brand-new'] = 'admin';
		expect((await access.check('me', 'github:acme/brand-new'))?.repo).toBe('github:acme/brand-new');
	});

	it('denies on a GitHub failure', async () => {
		const { access } = setup({ ...twoOrgs(), failPermission: true });
		expect(await access.check('me', 'github:acme/web')).toBeNull();
	});
});

describe('helpers', () => {
	it('installUrl', () => {
		expect(installUrl('my-app')).toBe('https://github.com/apps/my-app/installations/new');
	});
	it('groupByAccount', () => {
		const r = (repo: string, accountLogin: string, installationId: number) => ({
			repo,
			accountLogin,
			installationId,
			private: false,
			defaultBranch: 'main'
		});
		expect(
			groupByAccount([r('github:b/x', 'b', 2), r('github:a/x', 'a', 1), r('github:a/y', 'a', 1)])
		).toEqual([
			{ accountLogin: 'a', installationId: 1, repositoryCount: 2 },
			{ accountLogin: 'b', installationId: 2, repositoryCount: 1 }
		]);
	});
});

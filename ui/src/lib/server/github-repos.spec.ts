import { describe, expect, it, vi } from 'vitest';
import {
	RepoCreateError,
	addRepoToInstallation,
	createRepository,
	installationCoversAll,
	sameOwner,
	validateDescription,
	validateRepoName
} from './github-repos.ts';

const TOKEN = 'ghu_secret';
const input = {
	owner: 'acme',
	selfLogin: 'octo',
	name: 'my-repo',
	private: true,
	description: 'hello'
};
const created = {
	id: 7,
	full_name: 'acme/my-repo',
	html_url: 'https://github.com/acme/my-repo',
	private: true,
	default_branch: 'main'
};
const reply = (status: number, body: unknown = {}) =>
	new Response(JSON.stringify(body), { status });
const stub = (res: Response | (() => Response)) =>
	// eslint-disable-next-line @typescript-eslint/no-unused-vars
	vi.fn(async (_url: string | URL | Request, _init?: RequestInit) =>
		typeof res === 'function' ? res() : res
	);
const call = (f: ReturnType<typeof stub>, n = 0) => ({
	url: String(f.mock.calls[n][0]),
	init: f.mock.calls[n][1] as RequestInit
});

describe('validateRepoName', () => {
	it.each(['a', 'my-repo', 'a.b_c-1', 'x'.repeat(100)])('accepts %s', (n) =>
		expect(validateRepoName(n)).toBeNull()
	);
	it.each(['', '.', '..', 'a/b', 'a b', 'a\tb', 'x'.repeat(101), 'ä', 'a;b'])('rejects %j', (n) =>
		expect(validateRepoName(n)).not.toBeNull()
	);
});

describe('validateDescription', () => {
	it('accepts empty and 350 characters, rejects longer and control characters', () => {
		expect(validateDescription('')).toBeNull();
		expect(validateDescription('x'.repeat(350))).toBeNull();
		expect(validateDescription('x'.repeat(351))).not.toBeNull();
		expect(validateDescription('a\nb')).not.toBeNull();
		expect(validateDescription('a\u0000b')).not.toBeNull();
	});
});

describe('createRepository', () => {
	it('creates under an organization with auto_init and the chosen visibility', async () => {
		const f = stub(reply(201, created));
		const r = await createRepository(TOKEN, input, f);
		const { url, init } = call(f);
		expect(url).toBe('https://api.github.com/orgs/acme/repos');
		expect(init.method).toBe('POST');
		expect(init.redirect).toBe('error');
		expect((init.headers as Record<string, string>).Authorization).toBe(`Bearer ${TOKEN}`);
		expect(JSON.parse(init.body as string)).toEqual({
			name: 'my-repo',
			private: true,
			description: 'hello',
			auto_init: true
		});
		expect(r).toMatchObject({ id: 7, fullName: 'acme/my-repo', defaultBranch: 'main' });
	});

	it('sends public when asked', async () => {
		const f = stub(reply(201, created));
		await createRepository(TOKEN, { ...input, private: false }, f);
		expect(JSON.parse(call(f).init.body as string).private).toBe(false);
	});

	it('uses /user/repos for the own account, ignoring case', async () => {
		const f = stub(reply(201, created));
		await createRepository(TOKEN, { ...input, owner: 'Octo', selfLogin: 'octo' }, f);
		expect(call(f).url).toBe('https://api.github.com/user/repos');
		expect(sameOwner('Octo', 'octo')).toBe(true);
	});

	it('maps 403 and 404 to forbidden, with GitHub status and message, no second call', async () => {
		for (const status of [403, 404]) {
			const f = stub(reply(status, { message: 'Resource not accessible by integration' }));
			const err = await createRepository(TOKEN, input, f).catch((e) => e);
			expect(err).toBeInstanceOf(RepoCreateError);
			expect(err).toMatchObject({ code: 'forbidden', status });
			expect(err.message).toContain(String(status));
			expect(err.message).toContain('Resource not accessible by integration');
			expect(f).toHaveBeenCalledTimes(1);
		}
	});

	it('maps 422 name taken and other 422s', async () => {
		const taken = stub(
			reply(422, {
				message: 'Repository creation failed.',
				errors: [{ message: 'name already exists on this account' }]
			})
		);
		const err = await createRepository(TOKEN, input, taken).catch((e) => e);
		expect(err).toMatchObject({ code: 'name-taken', status: 422 });
		expect(err.message).toBe(
			'acme/my-repo already exists. Select it in the list below if you created it earlier.'
		);
		const other = stub(reply(422, { message: 'Validation Failed', errors: [{ message: 'bad' }] }));
		const err2 = await createRepository(TOKEN, input, other).catch((e) => e);
		expect(err2).toMatchObject({ code: 'invalid', status: 422 });
		expect(err2.message).toContain('Validation Failed');
	});

	it('maps a timeout or network failure to timeout', async () => {
		const f = vi.fn(async () => {
			throw new Error('aborted');
		});
		await expect(createRepository(TOKEN, input, f)).rejects.toMatchObject({ code: 'timeout' });
	});

	it('rejects invalid names without calling GitHub', async () => {
		for (const name of ['a/b', 'a b', '..', 'x'.repeat(101)]) {
			const f = stub(reply(201, created));
			await expect(createRepository(TOKEN, { ...input, name }, f)).rejects.toMatchObject({
				code: 'invalid'
			});
			expect(f).not.toHaveBeenCalled();
		}
	});
});

describe('addRepoToInstallation', () => {
	it('PUTs the repository to the installation with the same token', async () => {
		const f = stub(new Response(null, { status: 204 }));
		await addRepoToInstallation(TOKEN, 5, 7, f);
		const { url, init } = call(f);
		expect(url).toBe('https://api.github.com/user/installations/5/repositories/7');
		expect(init.method).toBe('PUT');
		expect((init.headers as Record<string, string>).Authorization).toBe(`Bearer ${TOKEN}`);
	});
	it('reports a refusal as created-not-installed', async () => {
		const f = stub(reply(403));
		await expect(addRepoToInstallation(TOKEN, 5, 7, f)).rejects.toMatchObject({
			code: 'created-not-installed',
			status: 403
		});
	});
});

describe('installationCoversAll', () => {
	const list = (selection: string) =>
		stub(reply(200, { installations: [{ id: 5, repository_selection: selection }] }));
	it('is true for all, false for selected, null when unknown', async () => {
		expect(await installationCoversAll(TOKEN, 5, list('all'))).toBe(true);
		expect(await installationCoversAll(TOKEN, 5, list('selected'))).toBe(false);
		expect(await installationCoversAll(TOKEN, 6, list('selected'))).toBeNull();
		expect(await installationCoversAll(TOKEN, 5, stub(reply(500)))).toBeNull();
	});
});

describe('GitHub API requests', () => {
	it('send the API URL, Bearer token and redirect: error on writes and reads', async () => {
		const w = stub(reply(201, created));
		await createRepository(TOKEN, input, w as unknown as typeof fetch);
		const r = stub(reply(200, { installations: [{ id: 5, repository_selection: 'all' }] }));
		expect(await installationCoversAll(TOKEN, 5, r as unknown as typeof fetch)).toBe(true);
		for (const f of [w, r]) {
			const { url, init } = call(f);
			expect(url).toMatch(/^https:\/\/api\.github\.com\//);
			expect(init.redirect).toBe('error');
			expect(init.headers).toMatchObject({
				Authorization: `Bearer ${TOKEN}`,
				Accept: 'application/vnd.github+json'
			});
		}
	});
});

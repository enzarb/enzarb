import { describe, expect, it, vi } from 'vitest';
import { coAuthorTrailer, commitRepoFile, readRepoFile } from './repo-files.ts';

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
const b64 = (s: string) => Buffer.from(s).toString('base64');

describe('readRepoFile', () => {
	it('decodes the file and returns its blob sha', async () => {
		const f = vi.fn(async () =>
			json(200, { type: 'file', encoding: 'base64', content: b64('x = 1\n'), sha: 'abc', size: 6 })
		);
		const r = await readRepoFile({
			repo: 'acme/w',
			path: '.ez/modes.toml',
			ref: 'main',
			token: 't',
			fetch: f as unknown as typeof fetch
		});
		expect(r).toEqual({ ok: true, file: { text: 'x = 1\n', sha: 'abc' } });
		const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toBe('https://api.github.com/repos/acme/w/contents/.ez/modes.toml?ref=main');
		expect((init.headers as Record<string, string>).Authorization).toBe('Bearer t');
	});

	it('is file: null on 404 and an error otherwise', async () => {
		const missing = await readRepoFile({
			repo: 'a/b',
			path: 'p',
			ref: 'main',
			token: 't',
			fetch: (async () => json(404, {})) as unknown as typeof fetch
		});
		expect(missing).toEqual({ ok: true, file: null });
		const dir = await readRepoFile({
			repo: 'a/b',
			path: 'p',
			ref: 'main',
			token: 't',
			fetch: (async () => json(200, [])) as unknown as typeof fetch
		});
		expect(dir.ok).toBe(false);
		const big = await readRepoFile({
			repo: 'a/b',
			path: 'p',
			ref: 'main',
			token: 't',
			fetch: (async () =>
				json(200, {
					type: 'file',
					sha: 's',
					size: 10_000_000,
					encoding: 'none',
					content: ''
				})) as unknown as typeof fetch
		});
		expect(big).toMatchObject({ ok: false, status: 413 });
	});
});

describe('commitRepoFile', () => {
	const base = {
		repo: 'acme/w',
		path: '.ez/modes.toml',
		branch: 'main',
		text: 'y = 2\n',
		message: 'm',
		token: 't'
	};

	it('PUTs base64 content with the replaced sha and returns the new shas', async () => {
		const f = vi.fn(async () =>
			json(200, {
				content: { sha: 'new' },
				commit: { sha: 'c1', html_url: 'https://github.com/acme/w/commit/c1' }
			})
		);
		const r = await commitRepoFile({ ...base, sha: 'old', fetch: f as unknown as typeof fetch });
		expect(r).toEqual({
			ok: true,
			sha: 'new',
			commitSha: 'c1',
			commitUrl: 'https://github.com/acme/w/commit/c1'
		});
		const init = (f.mock.calls[0] as unknown as [string, RequestInit])[1];
		expect(init.method).toBe('PUT');
		expect(JSON.parse(init.body as string)).toEqual({
			message: 'm',
			content: b64('y = 2\n'),
			branch: 'main',
			sha: 'old'
		});
	});

	it('omits sha when creating and maps refusals', async () => {
		const f = vi.fn(async () => json(201, { content: { sha: 'n' }, commit: { sha: 'c' } }));
		await commitRepoFile({ ...base, sha: null, fetch: f as unknown as typeof fetch });
		expect(
			JSON.parse((f.mock.calls[0] as unknown as [string, RequestInit])[1].body as string)
		).not.toHaveProperty('sha');
		for (const [status, reason] of [
			[409, 'conflict'],
			[422, 'conflict'],
			[403, 'forbidden'],
			[404, 'forbidden'],
			[500, 'failed']
		] as const) {
			const r = await commitRepoFile({
				...base,
				sha: 's',
				fetch: (async () => json(status, {})) as unknown as typeof fetch
			});
			expect(r).toMatchObject({ ok: false, reason, status });
		}
	});
});

it('builds a noreply co-author trailer', () => {
	expect(coAuthorTrailer({ login: 'octo', githubId: 42 })).toBe(
		'Co-authored-by: octo <42+octo@users.noreply.github.com>'
	);
});

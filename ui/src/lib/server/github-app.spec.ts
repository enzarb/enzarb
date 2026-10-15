import { generateKeyPairSync } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import { repoInstallationToken } from './github-app.ts';

const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
const pem = privateKey.export({ type: 'pkcs1', format: 'pem' }) as string;
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

describe('repoInstallationToken', () => {
	it('looks up the repository installation and mints a down-scoped token', async () => {
		const f = vi.fn(async (url: string) =>
			url.endsWith('/installation') ? json({ id: 9 }) : json({ token: 'ghs_x' }, 201)
		);
		const t = await repoInstallationToken(
			'acme/widgets',
			{ contents: 'write' },
			{ appId: '1', privateKey: pem, fetch: f as unknown as typeof fetch }
		);
		expect(t).toBe('ghs_x');
		expect(f.mock.calls[0][0]).toBe('https://api.github.com/repos/acme/widgets/installation');
		expect(f.mock.calls[1][0]).toBe('https://api.github.com/app/installations/9/access_tokens');
		expect(
			JSON.parse((f.mock.calls[1] as unknown as [string, RequestInit])[1].body as string)
		).toEqual({
			repositories: ['widgets'],
			permissions: { contents: 'write' }
		});
	});

	it('is null when unconfigured or not installed', async () => {
		const f = vi.fn(async () => json({}, 404));
		const opts = { appId: '1', privateKey: pem, fetch: f as unknown as typeof fetch };
		expect(await repoInstallationToken('a/b', {}, opts)).toBeNull();
		expect(await repoInstallationToken('a/b', {}, { ...opts, appId: undefined })).toBeNull();
	});
});

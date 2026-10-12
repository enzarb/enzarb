import { jwtVerify } from 'jose';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./kube.ts', () => ({ getProjectStore: () => ({}) }));
vi.mock('./project-cache.ts', () => ({
	projectCache: { list: vi.fn(async () => [{ name: 'alpha' }, { name: 'beta' }]) }
}));

const ENV = {
	DATABASE_URL: 'postgres://u:p@h:5432/db',
	ENZARB_SESSION_JWT_SECRET: 's',
	ENZARB_REALTIME_JWT_SECRET: 'rt-secret',
	GITHUB_OAUTH_CLIENT_ID: 'id',
	GITHUB_OAUTH_CLIENT_SECRET: 'secret',
	ENZARB_PUBLIC_URL: 'https://enzarb.example.com'
};

const cookies = { set: vi.fn(), delete: vi.fn() };

async function connect(user: { login: string } | null, legacy = '') {
	vi.resetModules();
	for (const [k, v] of Object.entries(ENV)) vi.stubEnv(k, v);
	vi.stubEnv('ENZARB_REALTIME_LEGACY_TOKEN_URL', legacy);
	const { issueRealtimeConnection } = await import('./realtime-connection.ts');
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	return issueRealtimeConnection({ locals: { user }, cookies } as any);
}

beforeEach(() => {
	vi.unstubAllEnvs();
	cookies.set.mockClear();
	cookies.delete.mockClear();
});

describe('issueRealtimeConnection', () => {
	it('sets the HttpOnly cookie with a correctly-claimed JWT and returns no token', async () => {
		const res = await connect({ login: 'octocat' });
		expect(cookies.set).toHaveBeenCalledTimes(1);
		const [name, token, opts] = cookies.set.mock.calls[0];
		expect(name).toBe('enzarb_live');
		expect(opts).toEqual({
			httpOnly: true,
			secure: true,
			sameSite: 'strict',
			path: '/live/',
			maxAge: 300
		});
		const { payload } = await jwtVerify(token, new TextEncoder().encode('rt-secret'), {
			audience: 'enzarb-realtime'
		});
		expect(payload.sub).toBe('octocat');
		expect(payload.projects).toEqual(['alpha', 'beta']);
		expect(payload.exp! - payload.iat!).toBe(opts.maxAge);
		expect(res).toEqual({ url: 'wss://enzarb.example.com/live/v1/stream', expiresInSeconds: 300 });
		expect(JSON.stringify(res)).not.toContain(token);
	});

	it('rejects without a session user and sets no cookie', async () => {
		await expect(connect(null)).rejects.toMatchObject({ status: 401 });
		expect(cookies.set).not.toHaveBeenCalled();
	});

	it('with the legacy flag on also puts the token in the URL and still sets the cookie', async () => {
		const res = await connect({ login: 'octocat' }, 'true');
		const token = cookies.set.mock.calls[0][1];
		expect(res.url).toBe(
			`wss://enzarb.example.com/live/v1/stream?token=${encodeURIComponent(token)}`
		);
	});
});

describe('legacy flag startup log', () => {
	it('warns once when enabled and never logs the token', async () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		await connect({ login: 'octocat' }, 'true');
		const { getConfig } = await import('./config.ts');
		getConfig();
		expect(warn).toHaveBeenCalledTimes(1);
		expect(warn.mock.calls[0][0]).not.toContain('token=');
		warn.mockRestore();
	});

	it('is silent when disabled', async () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		await connect({ login: 'octocat' });
		expect(warn).not.toHaveBeenCalled();
		warn.mockRestore();
	});
});

describe('sign-out', () => {
	it('expires enzarb_live with Path=/live/', async () => {
		vi.resetModules();
		for (const [k, v] of Object.entries(ENV)) vi.stubEnv(k, v);
		const { deleteSessionCookie } = await import('./auth.ts');
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		deleteSessionCookie(cookies as any);
		expect(cookies.delete).toHaveBeenCalledWith('enzarb_session', { path: '/' });
		expect(cookies.delete).toHaveBeenCalledWith('enzarb_live', { path: '/live/' });
	});
});

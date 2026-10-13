import { isHttpError } from '@sveltejs/kit';
import { jwtVerify } from 'jose';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { buildProject } from '#lib/projects.ts';
import type { ProjectStore } from '#lib/server/kube.ts';

// Only the cluster store and the request event are faked: the real projectService does the owner check.
const storeGet = vi.hoisted(() => vi.fn());
const locals = vi.hoisted(() => ({ user: undefined as unknown }));
vi.mock('#lib/server/kube.ts', () => ({
	getProjectStore: () => ({ get: storeGet }) as unknown as ProjectStore
}));
vi.mock('$app/server', () => ({
	// SvelteKit rejects remote-module exports without the `__.type` marker; the handler is otherwise as-is.
	command: (_schema: unknown, handler: object) =>
		Object.assign(handler, { __: { type: 'command' } }),
	getRequestEvent: () => ({ locals })
}));
vi.mock('#lib/server/config.ts', () => ({
	getConfig: () => ({ sessionJwtSecret: 'test-secret', publicUrl: 'https://enzarb.example' })
}));
const mint = vi.hoisted(() => ({ spy: vi.fn() }));
vi.mock('#lib/server/session-jwt.ts', async (orig) => {
	const real = await orig<typeof import('#lib/server/session-jwt.ts')>();
	mint.spy.mockImplementation(real.mintSessionToken);
	return { ...real, mintSessionToken: mint.spy };
});

import { chatConnection } from './chat.remote.ts';

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
const items = new Map<string, unknown>([['alpha', project('alpha', 'octo')]]);

beforeEach(() => {
	storeGet.mockReset();
	storeGet.mockImplementation(async (name: string) => items.get(name) ?? null);
	mint.spy.mockClear();
	locals.user = { id: 1, login: 'octo', name: null };
});

const call = chatConnection as unknown as (name: string) => Promise<{ url: string }>;
const status = async (p: Promise<unknown>) => {
	try {
		await p;
		return 200;
	} catch (e) {
		if (isHttpError(e)) return e.status;
		throw e;
	}
};

describe('chatConnection', () => {
	it('401s when signed out, without minting', async () => {
		locals.user = undefined;
		expect(await status(call('alpha'))).toBe(401);
		expect(mint.spy).not.toHaveBeenCalled();
	});

	it('404s for another user, the same as for a missing project, without minting', async () => {
		locals.user = { id: 2, login: 'mallory', name: null };
		expect(await status(call('alpha'))).toBe(404);
		locals.user = { id: 1, login: 'octo', name: null };
		expect(await status(call('beta'))).toBe(404);
		expect(mint.spy).not.toHaveBeenCalled();
	});

	it('404s for an over-long name without minting', async () => {
		expect(await status(call('a'.repeat(64)))).toBe(404);
		expect(mint.spy).not.toHaveBeenCalled();
	});

	it('gives the owner a url carrying a token for their login and project', async () => {
		const { url } = await call('alpha');
		expect(url).toContain('/sandbox/v1/projects/alpha/mux?token=');
		const token = new URL(url).searchParams.get('token')!;
		const { payload } = await jwtVerify(token, new TextEncoder().encode('test-secret'), {
			audience: 'enzarb-sandbox-proxy'
		});
		expect(payload.sub).toBe('octo');
		expect(payload.project).toBe('alpha');
	});
});

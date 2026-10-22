import { isHttpError } from '@sveltejs/kit';
import * as v from 'valibot';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { buildProject } from '#lib/projects.ts';
import type { ProjectStore } from '#lib/server/kube.ts';

// The cluster store, the request event, tracks and GitHub are faked; the real owner check and
// modes service run.
const storeGet = vi.hoisted(() => vi.fn());
const locals = vi.hoisted(() => ({ user: undefined as unknown }));
const tracks = vi.hoisted(() => ({ client: null as unknown }));
const check = vi.hoisted(() => vi.fn());
const refresh = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('#lib/server/kube.ts', () => ({
	getProjectStore: () => ({ get: storeGet }) as unknown as ProjectStore
}));
vi.mock('$app/server', () => ({
	command: (_schema: unknown, handler: object) =>
		Object.assign(handler, { __: { type: 'command' } }),
	query: (_schema: unknown, handler: (...a: unknown[]) => unknown) =>
		Object.assign((...a: unknown[]) => Object.assign(handler(...a) as object, { refresh }), {
			__: { type: 'query' }
		}),
	getRequestEvent: () => ({ locals })
}));
vi.mock('#lib/server/tracks.ts', async (orig) => ({
	...(await orig<typeof import('#lib/server/tracks.ts')>()),
	getTracks: () => tracks.client
}));
vi.mock('#lib/server/repo-access.ts', () => ({ getRepositoryAccess: () => ({ check }) }));
vi.mock('#lib/server/github-app.ts', () => ({
	token: vi.fn(async () => null),
	invalidateToken: vi.fn()
}));

import { SaveModesSchema } from './modes-schema.ts';
import { getModes, saveModesFile, validateModes } from './modes.remote.ts';

const project = (owner: string): Project =>
	buildProject(
		{
			name: 'p',
			displayName: '',
			repositories: [
				{ url: 'https://github.com/acme/w', connector: 'github', defaultBranch: 'main' }
			],
			triggers: { issues: false, changeRequests: false, mentions: false }
		},
		owner
	);
const run = (f: unknown, ...a: unknown[]) => (f as (...x: unknown[]) => Promise<unknown>)(...a);
const doc = { defaultMode: '', modes: [] };

beforeEach(() => {
	locals.user = { id: 1, githubId: 42, login: 'octo', name: null, avatarUrl: null };
	storeGet.mockReset();
	storeGet.mockResolvedValue(project('octo'));
	check.mockReset();
	tracks.client = {
		validateModes: vi.fn(async () => []),
		modesCatalog: vi.fn(),
		parseModes: vi.fn(),
		renderModes: vi.fn()
	};
});

describe('modes remote functions', () => {
	it('are 401 without a session and 404 for a foreign project', async () => {
		locals.user = undefined;
		expect(isHttpError(await run(validateModes, { name: 'p', doc }).catch((e) => e), 401)).toBe(
			true
		);
		locals.user = { id: 2, githubId: 7, login: 'mallory', name: null, avatarUrl: null };
		expect(isHttpError(await run(validateModes, { name: 'p', doc }).catch((e) => e), 404)).toBe(
			true
		);
		expect(
			isHttpError(
				await run(saveModesFile, { name: 'p', repo: 'github:acme/w', baseSha: null, doc }).catch(
					(e) => e
				),
				404
			)
		).toBe(true);
		expect(check).not.toHaveBeenCalled();
	});

	it('report the designer as unavailable without tracks', async () => {
		tracks.client = null;
		expect(await run(getModes, { name: 'p' })).toMatchObject({ ok: false, code: 'unavailable' });
		expect(await run(validateModes, { name: 'p', doc })).toMatchObject({
			ok: false,
			code: 'unavailable'
		});
	});

	it('validate through tracks', async () => {
		expect(await run(validateModes, { name: 'p', doc })).toEqual({ ok: true, issues: [] });
	});

	it('deny saving without live write access', async () => {
		check.mockResolvedValue(null);
		expect(
			await run(saveModesFile, { name: 'p', repo: 'github:acme/w', baseSha: null, doc })
		).toMatchObject({ ok: false, code: 'forbidden' });
		expect(check).toHaveBeenCalledWith('octo', 'github:acme/w');
	});
});

describe('SaveModesSchema', () => {
	const base = { name: 'p', repo: 'github:acme/w', baseSha: 'a'.repeat(40) };
	it('takes the form or the text, never both or neither', () => {
		expect(v.safeParse(SaveModesSchema, { ...base, doc }).success).toBe(true);
		expect(v.safeParse(SaveModesSchema, { ...base, text: 'x' }).success).toBe(true);
		expect(v.safeParse(SaveModesSchema, base).success).toBe(false);
		expect(v.safeParse(SaveModesSchema, { ...base, doc, text: 'x' }).success).toBe(false);
	});
	it('rejects bad shas and multi-line commit subjects', () => {
		expect(v.safeParse(SaveModesSchema, { ...base, baseSha: 'main', doc }).success).toBe(false);
		expect(
			v.safeParse(SaveModesSchema, { ...base, doc, message: 'a\nCo-authored-by: x' }).success
		).toBe(false);
	});
});

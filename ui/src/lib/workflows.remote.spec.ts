import { isHttpError } from '@sveltejs/kit';
import * as v from 'valibot';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project } from '#lib/project-types.ts';
import { buildProject } from '#lib/projects.ts';
import type { ProjectStore } from '#lib/server/kube.ts';

// The cluster store, the request event, Postgres, tracks and GitHub are faked; the real owner
// check and workflows service run.
const storeGet = vi.hoisted(() => vi.fn());
const locals = vi.hoisted(() => ({ user: undefined as unknown }));
const tracks = vi.hoisted(() => ({ client: null as unknown }));
const check = vi.hoisted(() => vi.fn());
const events = vi.hoisted(() => ({ list: vi.fn(), withPayloads: vi.fn() }));
const refresh = vi.hoisted(() => vi.fn(async () => {}));
const sync = vi.hoisted(() => ({
	list: [] as { arg: { name: string; repo?: string }; query: object; ignore: () => void }[],
	save: vi.fn(),
	loadWith: vi.fn()
}));
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
	getRequestEvent: () => ({ locals }),
	requested: () => sync.list
}));
vi.mock('#lib/server/tracks.ts', async (orig) => ({
	...(await orig<typeof import('#lib/server/tracks.ts')>()),
	getTracks: () => tracks.client
}));
vi.mock('#lib/server/db.ts', () => ({ getSql: () => ({}) }));
vi.mock('#lib/server/events-service.ts', () => ({ eventsService: () => events }));
vi.mock('#lib/server/repo-access.ts', () => ({ getRepositoryAccess: () => ({ check }) }));
vi.mock('#lib/server/github-app.ts', () => ({
	token: vi.fn(async () => null),
	invalidateToken: vi.fn()
}));

const real = await vi.importActual<typeof import('#lib/server/workflows-service.ts')>(
	'#lib/server/workflows-service.ts'
);
vi.mock('#lib/server/workflows-service.ts', async (orig) => ({
	...(await orig<typeof import('#lib/server/workflows-service.ts')>()),
	saveWorkflows: sync.save
}));

import { DryRunSchema, SaveWorkflowsSchema } from './workflows-schema.ts';
import {
	dryRunWorkflows,
	getWorkflows,
	listRepoEvents,
	renderWorkflowsFile,
	saveWorkflowsFile
} from './workflows.remote.ts';

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
const file = { version: 1 };

const entry = (repo?: string, name = 'p') => {
	const query = { set: vi.fn(), refresh: vi.fn(async () => {}) };
	const e = { arg: { name, repo }, query, ignore: vi.fn() };
	sync.list.push(e);
	return e;
};

beforeEach(() => {
	locals.user = { id: 1, githubId: 42, login: 'octo', name: null, avatarUrl: null };
	storeGet.mockReset();
	storeGet.mockResolvedValue(project('octo'));
	check.mockReset();
	sync.list.length = 0;
	sync.save.mockReset();
	sync.save.mockImplementation(real.saveWorkflows);
	events.list.mockReset();
	events.withPayloads.mockReset();
	tracks.client = {
		renderWorkflows: vi.fn(async () => ({ text: 'version = 1\n', diagnostics: [] })),
		dryRun: vi.fn(async () => ({ report: { diagnostics: [] }, results: [] }))
	};
});

describe('workflows remote functions', () => {
	it('are 404 for a foreign project', async () => {
		locals.user = { id: 2, githubId: 7, login: 'mallory', name: null, avatarUrl: null };
		const e = await run(renderWorkflowsFile, { name: 'p', file, modesToml: null }).catch((x) => x);
		expect(isHttpError(e, 404)).toBe(true);
		expect(
			isHttpError(
				await run(dryRunWorkflows, {
					name: 'p',
					repo: 'github:acme/w',
					text: '',
					modesToml: null,
					eventIds: [1]
				}).catch((x) => x),
				404
			)
		).toBe(true);
		expect(events.withPayloads).not.toHaveBeenCalled();
	});

	it('report the designer as unavailable without tracks', async () => {
		tracks.client = null;
		expect(await run(getWorkflows, { name: 'p' })).toMatchObject({ code: 'unavailable' });
	});

	it('render through tracks', async () => {
		expect(await run(renderWorkflowsFile, { name: 'p', file, modesToml: null })).toEqual({
			ok: true,
			text: 'version = 1\n',
			diagnostics: []
		});
	});

	it('deny saving without live write access', async () => {
		check.mockResolvedValue(null);
		expect(
			await run(saveWorkflowsFile, { name: 'p', repo: 'github:acme/w', baseSha: null, file })
		).toMatchObject({ ok: false, code: 'forbidden' });
	});

	describe('saving refreshes the page', () => {
		const loaded = { ok: true, repo: 'github:acme/w', repos: ['github:acme/w'], sha: 'new' };
		const saveArgs = { name: 'p', repo: 'github:acme/w', baseSha: null, file };

		it('sets the client query, default-repo and explicit keys alike, from the commit', async () => {
			sync.save.mockResolvedValue({
				ok: true,
				unchanged: false,
				sha: 'new',
				commitUrl: null,
				text: 't',
				report: [],
				loaded
			});
			const dflt = entry(undefined);
			const explicit = entry('github:ACME/w');
			const other = entry('github:acme/other');
			const foreign = entry('github:acme/w', 'q');
			const r = await run(saveWorkflowsFile, saveArgs);
			expect(r).not.toHaveProperty('loaded');
			expect(dflt.query.set).toHaveBeenCalledWith(loaded);
			expect(explicit.query.set).toHaveBeenCalledWith(loaded);
			expect(other.query.set).not.toHaveBeenCalled();
			expect(other.ignore).toHaveBeenCalled();
			expect(foreign.query.set).not.toHaveBeenCalled();
			expect(foreign.ignore).toHaveBeenCalled();
		});

		it('re-fetches when the committed content could not be assembled', async () => {
			sync.save.mockResolvedValue({
				ok: true,
				unchanged: false,
				sha: 'new',
				commitUrl: null,
				text: 't',
				report: []
			});
			const e = entry(undefined);
			await run(saveWorkflowsFile, saveArgs);
			expect(e.query.set).not.toHaveBeenCalled();
			expect(e.query.refresh).toHaveBeenCalled();
		});

		it('touches nothing after an unchanged or failed save', async () => {
			const e = entry(undefined);
			sync.save.mockResolvedValue({
				ok: true,
				unchanged: true,
				sha: 'old',
				commitUrl: null,
				text: 't',
				report: []
			});
			await run(saveWorkflowsFile, saveArgs);
			sync.save.mockResolvedValue({ ok: false, code: 'conflict', message: 'x' });
			await run(saveWorkflowsFile, saveArgs);
			expect(e.query.set).not.toHaveBeenCalled();
			expect(e.query.refresh).not.toHaveBeenCalled();
			expect(e.ignore).not.toHaveBeenCalled();
		});
	});

	it('list and dry-run only events of the chosen project repository', async () => {
		expect(await run(listRepoEvents, { name: 'p', repo: 'github:evil/x' })).toEqual([]);
		expect(events.list).not.toHaveBeenCalled();
		events.list.mockResolvedValue([]);
		await run(listRepoEvents, { name: 'p', repo: 'github:acme/w' });
		expect(events.list).toHaveBeenCalledWith(['acme/w'], undefined);

		check.mockResolvedValue(null);
		expect(
			await run(dryRunWorkflows, {
				name: 'p',
				repo: 'github:acme/w',
				text: 'version = 1',
				modesToml: null,
				eventIds: [3]
			})
		).toMatchObject({ ok: false, code: 'forbidden' });
		expect(events.withPayloads).not.toHaveBeenCalled();
		check.mockResolvedValue({ repo: 'github:acme/w', defaultBranch: 'main' });
		events.withPayloads.mockResolvedValue([{ id: 3, event: 'push', action: null, payload: {} }]);
		expect(
			await run(dryRunWorkflows, {
				name: 'p',
				repo: 'github:acme/w',
				text: 'version = 1',
				modesToml: null,
				eventIds: [3]
			})
		).toMatchObject({ ok: true, missing: [] });
		expect(events.withPayloads).toHaveBeenCalledWith(['acme/w'], [3]);
	});
});

describe('schemas', () => {
	const base = { name: 'p', repo: 'github:acme/w', baseSha: null };
	it('save takes the graph or the text, never both', () => {
		expect(v.safeParse(SaveWorkflowsSchema, { ...base, file }).success).toBe(true);
		expect(v.safeParse(SaveWorkflowsSchema, { ...base, text: 'x' }).success).toBe(true);
		expect(v.safeParse(SaveWorkflowsSchema, { ...base, file, text: 'x' }).success).toBe(false);
		expect(v.safeParse(SaveWorkflowsSchema, base).success).toBe(false);
	});
	it('dry runs take 1-50 events', () => {
		const d = { name: 'p', repo: 'r', text: '', modesToml: null };
		expect(v.safeParse(DryRunSchema, { ...d, eventIds: [] }).success).toBe(false);
		expect(
			v.safeParse(DryRunSchema, { ...d, eventIds: Array.from({ length: 51 }, (_, i) => i + 1) })
				.success
		).toBe(false);
		expect(v.safeParse(DryRunSchema, { ...d, eventIds: [1] }).success).toBe(true);
	});
});

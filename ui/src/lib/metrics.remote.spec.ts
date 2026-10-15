import { isHttpError } from '@sveltejs/kit';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { buildProject } from '#lib/projects.ts';
import type { ProjectStore } from '#lib/server/kube.ts';

const storeGet = vi.hoisted(() => vi.fn());
const locals = vi.hoisted(() => ({ user: undefined as unknown }));
const usage = vi.hoisted(() => vi.fn());
vi.mock('#lib/server/kube.ts', () => ({
	getProjectStore: () => ({ get: storeGet }) as unknown as ProjectStore
}));
vi.mock('#lib/server/db.ts', () => ({ getSql: () => 'sql' }));
vi.mock('#lib/server/metrics-service.ts', () => ({ projectUsage: usage }));
vi.mock('$app/server', () => ({
	query: (_schema: unknown, handler: object) => Object.assign(handler, { __: { type: 'query' } }),
	getRequestEvent: () => ({ locals })
}));

import { getProjectUsage } from './metrics.remote.ts';

const mine = buildProject(
	{
		name: 'alpha',
		displayName: '',
		repositories: [],
		triggers: { issues: false, changeRequests: false, mentions: false }
	},
	'octo'
);

type Args = { name: string; window?: string; from?: string; to?: string };
const call = getProjectUsage as unknown as (a: Args) => Promise<unknown>;
const status = async (p: Promise<unknown>) => {
	try {
		await p;
		return 200;
	} catch (e) {
		if (isHttpError(e)) return e.status;
		throw e;
	}
};

beforeEach(() => {
	storeGet.mockReset();
	storeGet.mockImplementation(async (name: string) => (name === 'alpha' ? mine : null));
	usage.mockReset();
	usage.mockResolvedValue({ total: {}, tasks: [] });
	locals.user = { id: 1, login: 'octo', name: null };
});

describe('getProjectUsage', () => {
	it('is 401 when signed out and queries nothing', async () => {
		locals.user = undefined;
		expect(await status(call({ name: 'alpha' }))).toBe(401);
		expect(usage).not.toHaveBeenCalled();
	});

	it('is 404 for a missing or foreign project and queries nothing', async () => {
		expect(await status(call({ name: 'nope' }))).toBe(404);
		locals.user = { id: 2, login: 'other', name: null };
		expect(await status(call({ name: 'alpha' }))).toBe(404);
		expect(usage).not.toHaveBeenCalled();
	});

	it('is 400 for a bad window', async () => {
		expect(await status(call({ name: 'alpha', window: '5y' }))).toBe(400);
		expect(await status(call({ name: 'alpha', from: '2020-01-01', to: '2026-01-01' }))).toBe(400);
		expect(usage).not.toHaveBeenCalled();
	});

	it('queries the default window for the owner', async () => {
		await call({ name: 'alpha' });
		expect(usage).toHaveBeenCalledWith('sql', 'alpha', expect.objectContaining({ preset: '7d' }));
	});
});

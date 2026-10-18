import { beforeEach, describe, expect, it, vi } from 'vitest';

const versions = vi.hoisted(() => ({
	value: {
		chart: '0.4.1' as string | undefined,
		components: {} as Record<string, string>
	}
}));
const releases = vi.hoisted(() => ({ fn: vi.fn() }));
vi.mock('#lib/server/config.ts', () => ({
	getConfig: () => ({ componentVersions: versions.value })
}));
const tok = vi.hoisted(() => ({ get: vi.fn(), invalidate: vi.fn() }));
vi.mock('#lib/server/github-app.ts', () => ({
	token: tok.get,
	invalidateToken: tok.invalidate
}));
vi.mock('#lib/server/releases.ts', () => ({
	RELEASES_REPO: 'enzarb/enzarb',
	cachedReleases: releases.fn
}));
const store = vi.hoisted(() => ({ list: vi.fn() }));
vi.mock('#lib/server/kube.ts', () => ({ getProjectStore: () => ({ list: store.list }) }));
const rollout = vi.hoisted(() => ({ fn: vi.fn() }));
vi.mock('#lib/server/updater-status.ts', () => ({ fetchChartRollout: rollout.fn }));

import { load } from './+page.server.ts';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const run = (user: unknown) => (load as (e: unknown) => Promise<any>)({ locals: { user } });
const user = { id: 1, login: 'octo', name: null };
const release = (tag: string, body = 'hi') => ({
	tag,
	name: tag,
	publishedAt: '2026-10-01T00:00:00Z',
	url: `https://github.com/x/releases/${tag}`,
	body
});

beforeEach(() => {
	releases.fn.mockReset();
	tok.get.mockReset();
	tok.get.mockResolvedValue(null);
	rollout.fn.mockReset();
	rollout.fn.mockResolvedValue(null);
	store.list.mockReset();
	store.list.mockResolvedValue([]);
	versions.value = {
		chart: '0.4.1',
		components: { 'enzarb-ui': 'v1.0.0', 'enzarb-extra': '2.0.0' }
	};
});

describe('about load', () => {
	it('redirects when signed out', async () => {
		await expect(run(null)).rejects.toMatchObject({ status: 303, location: '/login' });
	});

	it('returns sanitized release html and unknown for missing versions', async () => {
		releases.fn.mockResolvedValue([release('v1.0.0', '<script>x</script>\n\n**b**')]);
		const data = await run(user);
		expect(data.chart).toBe('0.4.1');
		const ui = data.components.find((c: { name: string }) => c.name === 'enzarb-ui');
		expect(ui.deployed).toBe('v1.0.0');
		expect(ui.releases[0].html).toContain('<strong>b</strong>');
		expect(ui.releases[0].html).not.toContain('<script');
		const op = data.components.find((c: { name: string }) => c.name === 'enzarb-sandbox-operator');
		expect(op.deployed).toBe('unknown');
		const extra = data.components.find((c: { name: string }) => c.name === 'enzarb-extra');
		expect(extra).toMatchObject({ deployed: '2.0.0', repoUrl: null, releases: null });
		expect(ui.repoUrl).toBe('https://github.com/enzarb/enzarb/tree/main/ui');
		expect(releases.fn).toHaveBeenCalledWith('ui', expect.anything());
		expect(releases.fn).toHaveBeenCalledWith('operator', expect.anything());
	});

	it('passes the installation token to the fetch and keeps it out of the page data', async () => {
		tok.get.mockResolvedValue('ghs_secret');
		releases.fn.mockResolvedValue([release('v1.0.0')]);
		const data = await run(user);
		expect(releases.fn).toHaveBeenCalledWith(
			'ui',
			expect.objectContaining({ token: 'ghs_secret', onTokenRejected: expect.any(Function) })
		);
		expect(tok.get).toHaveBeenCalledWith('enzarb/enzarb', { contents: 'read' });
		releases.fn.mock.calls[0][1].onTokenRejected();
		expect(tok.invalidate).toHaveBeenCalledWith('enzarb/enzarb', { contents: 'read' });
		expect(JSON.stringify(data)).not.toContain('ghs_secret');
	});

	it('fetches unauthenticated when the token lookup fails', async () => {
		tok.get.mockRejectedValue(new Error('boom'));
		releases.fn.mockResolvedValue([release('v1.0.0')]);
		const data = await run(user);
		expect(releases.fn).toHaveBeenCalledWith('ui', expect.objectContaining({ token: undefined }));
		expect(data.components[0].releases).toHaveLength(1);
	});

	it('includes the updater chart rollout, or null when unavailable', async () => {
		releases.fn.mockResolvedValue([]);
		const status = {
			installed: '0.9.0',
			desired: '0.10.0',
			latest: '0.10.0',
			stuck: true,
			releases: []
		};
		rollout.fn.mockResolvedValue(status);
		expect((await run(user)).chartRollout).toEqual(status);
		rollout.fn.mockResolvedValue(null);
		expect((await run(user)).chartRollout).toBeNull();
	});

	it('still returns versions when one component fetch fails', async () => {
		releases.fn.mockImplementation(async (dir: string) => {
			if (dir === 'ui') throw new Error('HTTP 403');
			return [release('v1')];
		});
		const data = await run(user);
		const ui = data.components.find((c: { name: string }) => c.name === 'enzarb-ui');
		expect(ui).toMatchObject({ deployed: 'v1.0.0', releases: null });
		const op = data.components.find((c: { name: string }) => c.name === 'enzarb-sandbox-operator');
		expect(op.releases).toHaveLength(1);
	});

	it('still returns versions when every fetch fails', async () => {
		releases.fn.mockRejectedValue(new Error('offline'));
		const data = await run(user);
		expect(data.components.length).toBeGreaterThan(1);
		expect(data.components.every((c: { releases: unknown }) => c.releases === null)).toBe(true);
		expect(data.components[0].deployed).toBe('v1.0.0');
	});

	it('joins helm releases, the updater history and project rolls into releaseTimings', async () => {
		releases.fn.mockImplementation(async (dir: string) =>
			dir === 'helm' ? [{ ...release('v0.9.0'), publishedAt: '2026-10-01T10:00:00Z' }] : []
		);
		rollout.fn.mockResolvedValue({
			installed: '0.9.0',
			desired: '0.9.0',
			latest: '0.9.0',
			stuck: false,
			releases: [{ version: '0.9.0', deployedAt: '2026-10-01T10:03:00Z' }]
		});
		store.list.mockResolvedValue([
			{ status: { lastRoll: { at: '2026-10-01T10:20:00Z', reason: 'r' } } },
			{ status: null }
		]);
		const data = await run(user);
		expect(data.releaseTimings).toHaveLength(1);
		expect(data.releaseTimings[0]).toMatchObject({
			version: 'v0.9.0',
			mergeToDeployedSeconds: 180,
			sandboxAppliedAt: '2026-10-01T10:20:00.000Z'
		});
	});

	it('returns no timings when the sources fail', async () => {
		releases.fn.mockRejectedValue(new Error('offline'));
		store.list.mockRejectedValue(new Error('forbidden'));
		expect((await run(user)).releaseTimings).toEqual([]);
	});
});

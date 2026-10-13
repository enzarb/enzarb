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
vi.mock('#lib/server/releases.ts', () => ({ cachedReleases: releases.fn }));

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
		expect(releases.fn).toHaveBeenCalledWith('ui');
		expect(releases.fn).toHaveBeenCalledWith('operator');
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
});

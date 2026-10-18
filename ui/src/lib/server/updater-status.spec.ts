import { describe, expect, it } from 'vitest';
import { fetchChartRollout } from './updater-status.ts';

const json = (body: unknown, status = 200) =>
	(async () => new Response(JSON.stringify(body), { status })) as unknown as typeof fetch;

describe('fetchChartRollout', () => {
	it('maps the updater status', async () => {
		const r = await fetchChartRollout({
			url: 'http://u/status',
			fetch: json({ installed: '0.9.0', desired: '0.10.0', latest: '0.10.0', stuck: true })
		});
		expect(r).toEqual({
			installed: '0.9.0',
			desired: '0.10.0',
			latest: '0.10.0',
			stuck: true,
			releases: []
		});
	});

	it('parses the deploy history leniently', async () => {
		const r = await fetchChartRollout({
			fetch: json({
				stuck: false,
				releases: [
					{
						version: '0.9.0',
						deployedAt: '2026-10-01T10:03:00Z',
						sandboxImageVersion: '0.34.0'
					},
					{ version: '', deployedAt: '2026-10-01T10:03:00Z' },
					{ version: '0.8.0', deployedAt: 'yesterday' },
					'junk',
					null
				]
			})
		});
		expect(r?.releases).toEqual([
			{ version: '0.9.0', deployedAt: '2026-10-01T10:03:00Z', sandboxImageVersion: '0.34.0' }
		]);
		const bad = await fetchChartRollout({ fetch: json({ stuck: false, releases: 'x' }) });
		expect(bad?.releases).toEqual([]);
	});

	it('turns nulls and junk values into null', async () => {
		const r = await fetchChartRollout({
			fetch: json({ installed: null, desired: 3, latest: '', stuck: false })
		});
		expect(r).toEqual({ installed: null, desired: null, latest: null, stuck: false, releases: [] });
	});

	it('is null for an old updater (404), a bad body or a network error', async () => {
		expect(await fetchChartRollout({ fetch: json({}, 404) })).toBeNull();
		expect(await fetchChartRollout({ fetch: json({ installed: '1' }) })).toBeNull();
		expect(await fetchChartRollout({ fetch: json('x') })).toBeNull();
		const boom = (async () => {
			throw new Error('ECONNREFUSED');
		}) as unknown as typeof fetch;
		expect(await fetchChartRollout({ fetch: boom })).toBeNull();
	});
});

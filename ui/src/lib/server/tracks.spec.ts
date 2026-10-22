import { describe, expect, it, vi } from 'vitest';
import { TracksError, createTracksClient } from './tracks.ts';

const cfg = { url: 'http://tracks:8080', token: 'x'.repeat(32) };
const json = (status: number, body: unknown) =>
	new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

describe('tracks client', () => {
	it('sends the bearer token and JSON body to the route', async () => {
		const f = vi.fn(async () => json(200, { issues: [] }));
		const c = createTracksClient(cfg, f as unknown as typeof fetch);
		expect(await c.validateModes({ defaultMode: 'a', modes: [] })).toEqual([]);
		const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toBe('http://tracks:8080/v1/modes/validate');
		expect(init.method).toBe('POST');
		expect((init.headers as Record<string, string>).Authorization).toBe(`Bearer ${cfg.token}`);
		expect(JSON.parse(init.body as string)).toEqual({ doc: { defaultMode: 'a', modes: [] } });
	});

	it('GETs the catalog and omits `old` for a new file', async () => {
		const f = vi.fn(async () => json(200, { text: 't', issues: [], rewritten: [] }));
		const c = createTracksClient(cfg, f as unknown as typeof fetch);
		await c.renderModes({ defaultMode: '', modes: [] }, null);
		expect(
			JSON.parse((f.mock.calls[0] as unknown as [string, RequestInit])[1].body as string)
		).toEqual({
			doc: { defaultMode: '', modes: [] }
		});
		f.mockResolvedValueOnce(json(200, { builtinTools: [] }));
		await c.modesCatalog();
		expect((f.mock.calls[1] as unknown as [string, RequestInit])[1].method).toBe('GET');
	});

	it('turns error envelopes into TracksError with issues', async () => {
		const issues = [{ path: 'defaultMode', message: 'unknown', severity: 'error' }];
		const f = vi.fn(async () =>
			json(422, { error: { code: 'invalid_modes', message: 'the modes have errors' }, issues })
		);
		const c = createTracksClient(cfg, f as unknown as typeof fetch);
		const err = (await c
			.renderModes({ defaultMode: 'x', modes: [] }, 'old')
			.catch((e) => e)) as TracksError;
		expect(err).toBeInstanceOf(TracksError);
		expect([err.status, err.code, err.message]).toEqual([
			422,
			'invalid_modes',
			'the modes have errors'
		]);
		expect(err.issues).toEqual(issues);
	});

	it('reports an unreachable or non-JSON tracks as unavailable without the token', async () => {
		const down = createTracksClient(cfg, (async () => {
			throw new Error(`connect ECONNREFUSED ${cfg.token}`);
		}) as unknown as typeof fetch);
		const e1 = (await down.modesCatalog().catch((e) => e)) as TracksError;
		expect([e1.status, e1.code]).toEqual([503, 'unavailable']);
		expect(e1.message).not.toContain(cfg.token);
		const html = createTracksClient(
			cfg,
			(async () => new Response('<html>', { status: 502 })) as unknown as typeof fetch
		);
		const e2 = (await html.modesCatalog().catch((e) => e)) as TracksError;
		expect([e2.status, e2.code]).toEqual([502, 'unavailable']);
	});
});

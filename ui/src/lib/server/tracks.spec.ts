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

	it('maps the workflows routes to their snake_case bodies and flattens the report', async () => {
		const diagnostics = [{ severity: 'warning', path: 'workflows.a', message: 'm' }];
		const f = vi.fn(async () => json(200, { report: { diagnostics }, file: { version: 1 } }));
		const c = createTracksClient(cfg, f as unknown as typeof fetch);
		const body = (n: number) =>
			JSON.parse((f.mock.calls[n] as unknown as [string, RequestInit])[1].body as string);
		const url = (n: number) => (f.mock.calls[n] as unknown as [string])[0];

		expect(await c.checkWorkflows('version = 1', null)).toEqual({
			diagnostics,
			file: { version: 1 }
		});
		expect(url(0)).toBe('http://tracks:8080/v1/workflows/check');
		expect(body(0)).toEqual({ workflows_toml: 'version = 1' });

		f.mockResolvedValueOnce(json(200, { text: 'version = 1\n', report: { diagnostics: [] } }));
		expect(await c.renderWorkflows({ version: 1 }, '[modes.a]')).toEqual({
			text: 'version = 1\n',
			diagnostics: []
		});
		expect(body(1)).toEqual({ file: { version: 1 }, modes_toml: '[modes.a]' });

		f.mockResolvedValueOnce(json(200, { report: { diagnostics: [] }, results: [] }));
		const events = [{ id: 7, event: 'issues', action: 'labeled', payload: {} }];
		await c.dryRun({ text: 't', modes: null, events, options: { assume: { a: {} } } });
		expect(url(2)).toBe('http://tracks:8080/v1/dry-run');
		expect(body(2)).toEqual({ workflows_toml: 't', events, options: { assume: { a: {} } } });
	});

	it('reads decisions and maps them to camelCase', async () => {
		const f = vi.fn(async () =>
			json(200, {
				decisions: [
					{
						event_id: 7,
						repo: 'o/r',
						project: 'p',
						sha: 'abc1234',
						outcome: 'started',
						matched: ['fix'],
						reasons: [{ workflow: 'fix', matched: true, on: 'issues', reason: 'on `issues`' }],
						errors: [],
						resumed: [{ run: 'r1', workflow: 'fix', node: 'w' }],
						decided_at: '2026-10-30T00:00:00.000Z'
					}
				]
			})
		);
		const c = createTracksClient(cfg, f as unknown as typeof fetch);
		expect(await c.decisions([])).toEqual([]);
		expect(f).not.toHaveBeenCalled();
		const [d] = await c.decisions([7, 8]);
		expect((f.mock.calls[0] as unknown as [string, RequestInit])[0]).toBe(
			'http://tracks:8080/v1/decisions?event_ids=7,8'
		);
		expect(d).toMatchObject({ eventId: 7, outcome: 'started', started: ['fix'], sha: 'abc1234' });
		expect(d.resumed).toEqual([{ run: 'r1', workflow: 'fix', node: 'w' }]);
		expect(d.reasons[0]).toEqual({
			workflow: 'fix',
			matched: true,
			on: 'issues',
			reason: 'on `issues`'
		});
	});

	it('starts a run by hand and treats a live run as a normal answer', async () => {
		const f = vi.fn(async () => json(200, { run: { id: 'r1' } }));
		const c = createTracksClient(cfg, f as unknown as typeof fetch);
		const input = { repo: 'o/r', number: 4, workflow: 'implement', actor: 'bob' };
		expect(await c.startRun(input)).toEqual({ id: 'r1', alreadyRunning: false });
		const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
		expect(url).toBe('http://tracks:8080/v1/runs');
		expect(init.method).toBe('POST');
		expect(JSON.parse(init.body as string)).toEqual(input);
		f.mockResolvedValueOnce(json(409, { error: { code: 'already_running', message: 'busy' } }));
		expect(await c.startRun(input)).toEqual({ id: null, alreadyRunning: true });
		f.mockResolvedValueOnce(json(422, { error: { code: 'not_startable', message: 'no' } }));
		await expect(c.startRun(input)).rejects.toMatchObject({ status: 422, code: 'not_startable' });
	});
});

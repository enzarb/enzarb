import { describe, expect, it } from 'vitest';
import { defaultWindow, parseWindow, windowToParams } from './metrics-window.ts';

const now = new Date('2026-10-15T12:00:00Z');
const parse = (q: string) => parseWindow(new URLSearchParams(q), now);
const ok = (q: string) => {
	const r = parse(q);
	if (!r.ok) throw new Error(r.error.message);
	return r.window;
};
const code = (q: string) => {
	const r = parse(q);
	return r.ok ? null : r.error.code;
};

describe('parseWindow', () => {
	it('defaults to the last 7 days', () => {
		const w = ok('');
		expect(w.preset).toBe('7d');
		expect(w.to).toEqual(now);
		expect(w.from).toEqual(new Date('2026-10-08T12:00:00Z'));
		expect(w).toEqual(defaultWindow(now));
	});

	it.each([
		['24h', '2026-10-14T12:00:00Z'],
		['7d', '2026-10-08T12:00:00Z'],
		['30d', '2026-09-15T12:00:00Z']
	])('maps the %s preset', (p, from) => {
		const w = ok(`window=${p}`);
		expect(w.from).toEqual(new Date(from));
		expect(w.to).toEqual(now);
		expect(w.preset).toBe(p);
	});

	it('rejects an unknown preset', () => {
		expect(code('window=1y')).toBe('preset');
	});

	it('reads a date range with the end date covering its whole day', () => {
		const w = ok('from=2026-10-01&to=2026-10-07');
		expect(w.preset).toBe('custom');
		expect(w.from).toEqual(new Date('2026-10-01T00:00:00Z'));
		expect(w.to).toEqual(new Date('2026-10-08T00:00:00Z'));
	});

	it('reads datetimes with an offset', () => {
		const w = ok('from=2026-10-01T08:00:00%2B02:00&to=2026-10-02T08:00Z');
		expect(w.from).toEqual(new Date('2026-10-01T06:00:00Z'));
		expect(w.to).toEqual(new Date('2026-10-02T08:00:00Z'));
	});

	it('rejects a half-given range', () => {
		expect(code('from=2026-10-01')).toBe('missing');
		expect(code('to=2026-10-01')).toBe('missing');
	});

	it.each(['yesterday', '2026-02-31', '2026-10-01T08:00', '1'])('rejects %s', (bad) => {
		expect(code(`from=${bad}&to=2026-10-07`)).toBe('unparsable');
	});

	it('rejects from >= to', () => {
		expect(code('from=2026-10-07T00:00Z&to=2026-10-07T00:00Z')).toBe('order');
		expect(code('from=2026-10-08&to=2026-10-01')).toBe('order');
	});

	it('limits the span to 366 days', () => {
		expect(code('from=2025-10-14&to=2026-10-14')).toBeNull();
		expect(code('from=2025-10-01&to=2026-10-14')).toBe('span');
	});

	it('rejects an end in the future but allows today and a little skew', () => {
		expect(code('from=2026-10-10&to=2026-10-15')).toBeNull();
		expect(code('from=2026-10-10&to=2026-10-16')).toBe('future');
		expect(code('from=2026-10-10T00:00Z&to=2026-10-15T12:03:00Z')).toBeNull();
		expect(code('from=2026-10-10T00:00Z&to=2026-10-15T13:00:00Z')).toBe('future');
	});
});

describe('windowToParams', () => {
	it('writes a preset as ?window=', () => {
		expect(windowToParams(ok('window=30d')).toString()).toBe('window=30d');
	});

	it.each([
		'from=2026-10-01&to=2026-10-07',
		'from=2026-10-01T08%3A30%3A00.000Z&to=2026-10-02T09%3A00%3A00.000Z'
	])('round-trips a custom range %s', (q) => {
		const w = ok(q);
		expect(windowToParams(w).toString()).toBe(q);
		const again = parseWindow(windowToParams(w), now);
		expect(again).toEqual({ ok: true, window: w });
	});
});

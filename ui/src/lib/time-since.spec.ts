import { describe, expect, it } from 'vitest';
import { timeSince } from './time-since.ts';

const NOW = Date.parse('2026-10-14T12:00:00Z');
const ago = (ms: number) => new Date(NOW - ms).toISOString();
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

describe('timeSince', () => {
	it('says just now below a minute', () => {
		expect(timeSince(ago(0), NOW)).toBe('just now');
		expect(timeSince(ago(59_999), NOW)).toBe('just now');
	});

	it('counts minutes up to an hour', () => {
		expect(timeSince(ago(MIN), NOW)).toBe('1m ago');
		expect(timeSince(ago(59 * MIN + 59_000), NOW)).toBe('59m ago');
	});

	it('counts hours up to a day', () => {
		expect(timeSince(ago(HOUR), NOW)).toBe('1h ago');
		expect(timeSince(ago(23 * HOUR + 59 * MIN), NOW)).toBe('23h ago');
	});

	it('counts days up to 30', () => {
		expect(timeSince(ago(DAY), NOW)).toBe('1d ago');
		expect(timeSince(ago(29 * DAY + 23 * HOUR), NOW)).toBe('29d ago');
	});

	it('shows the date from 30 days on', () => {
		expect(timeSince('2026-09-14T12:00:00Z', NOW)).toBe('2026-09-14');
		expect(timeSince('2025-01-02T03:04:05Z', NOW)).toBe('2025-01-02');
	});

	it('clamps the future to just now', () => {
		expect(timeSince(new Date(NOW + 5 * HOUR).toISOString(), NOW)).toBe('just now');
	});

	it('does not invent an age for garbage', () => {
		expect(timeSince('nonsense', NOW)).toBe('unknown');
	});
});

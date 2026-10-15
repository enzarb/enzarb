import { describe, expect, it } from 'vitest';
import { formatDuration } from './duration.ts';

describe('formatDuration', () => {
	it('picks the unit by size', () => {
		expect(formatDuration(0.25)).toBe('250ms');
		expect(formatDuration(42)).toBe('42s');
		expect(formatDuration(185)).toBe('3m 05s');
		expect(formatDuration(7800)).toBe('2h 10m');
		expect(formatDuration(100_800)).toBe('1d 4h');
	});

	it('reads a missing or invalid value as a dash', () => {
		for (const x of [null, undefined, NaN, Infinity, -1]) expect(formatDuration(x)).toBe('–');
	});
});

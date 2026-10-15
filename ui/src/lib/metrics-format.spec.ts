import { describe, expect, it } from 'vitest';
import { formatBytes, formatBytesPerSec, formatCores, NOT_REPORTED } from './metrics-format.ts';

describe('formatBytes', () => {
	it('switches unit at 1024', () => {
		expect(formatBytes(0)).toBe('0 B');
		expect(formatBytes(1023)).toBe('1023 B');
		expect(formatBytes(1024)).toBe('1 KiB');
		expect(formatBytes(1536)).toBe('1.5 KiB');
		expect(formatBytes(1024 ** 2)).toBe('1 MiB');
		expect(formatBytes(5 * 1024 ** 3)).toBe('5 GiB');
		expect(formatBytes(200 * 1024 ** 2)).toBe('200 MiB');
	});

	it('reads null, NaN and negatives as not reported', () => {
		expect(formatBytes(null)).toBe(NOT_REPORTED);
		expect(formatBytes(NaN)).toBe(NOT_REPORTED);
		expect(formatBytes(-1)).toBe(NOT_REPORTED);
	});
});

describe('formatBytesPerSec', () => {
	it('adds the rate suffix', () => {
		expect(formatBytesPerSec(2048)).toBe('2 KiB/s');
		expect(formatBytesPerSec(null)).toBe(NOT_REPORTED);
	});
});

describe('formatCores', () => {
	it('shows cores with trimmed decimals', () => {
		expect(formatCores(0)).toBe('0 cores');
		expect(formatCores(0.25)).toBe('0.25 cores');
		expect(formatCores(1)).toBe('1 core');
		expect(formatCores(2.5)).toBe('2.5 cores');
		expect(formatCores(12)).toBe('12 cores');
		expect(formatCores(null)).toBe(NOT_REPORTED);
		expect(formatCores(NaN)).toBe(NOT_REPORTED);
	});
});

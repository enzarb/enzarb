import { describe, expect, it } from 'vitest';
import { displayVersion, matchRelease, normalizeTag } from './versions.ts';

const releases = [{ tag: 'v1.2.3' }, { tag: 'v1.2.2' }, { tag: '1.0.0' }];

describe('normalizeTag', () => {
	it('strips whitespace and a leading v', () => {
		expect(normalizeTag(' v1.2.3 ')).toBe('1.2.3');
		expect(normalizeTag('1.2.3')).toBe('1.2.3');
	});
});

describe('matchRelease', () => {
	it('matches exactly regardless of the v prefix', () => {
		expect(matchRelease('v1.2.2', releases)).toBe(1);
		expect(matchRelease('1.2.3', releases)).toBe(0);
		expect(matchRelease('v1.0.0', releases)).toBe(2);
	});
	it('does not match prefixes or non-release tags', () => {
		expect(matchRelease('1.2', releases)).toBe(-1);
		expect(matchRelease('main-abc1234', releases)).toBe(-1);
		expect(matchRelease('0.1.0-main.9', releases)).toBe(-1);
	});
	it('matches nothing when empty or missing', () => {
		expect(matchRelease('', releases)).toBe(-1);
		expect(matchRelease(undefined, releases)).toBe(-1);
	});
});

describe('displayVersion', () => {
	it('shows unknown for empty or missing', () => {
		expect(displayVersion(undefined)).toBe('unknown');
		expect(displayVersion('  ')).toBe('unknown');
		expect(displayVersion(' v1.0.0 ')).toBe('v1.0.0');
	});
});

import { describe, expect, it } from 'vitest';
import { safeReturnPath } from './return-path.ts';

describe('safeReturnPath', () => {
	it('keeps same-origin relative paths with query and hash', () => {
		expect(safeReturnPath('/projects')).toBe('/projects');
		expect(safeReturnPath('/projects/a?tab=x#top')).toBe('/projects/a?tab=x#top');
	});
	it('falls back to / for anything else', () => {
		for (const bad of [
			null,
			undefined,
			'',
			'projects',
			'//evil.example',
			'/\\evil.example',
			'https://evil.example/x',
			'javascript:alert(1)',
			'/a\nb',
			'/login',
			'/login/github?refresh=1',
			'/.//evil.example',
			'/..//evil.example',
			'/a/..//evil.example',
			'/%2e%2e//evil',
			'/' + 'a'.repeat(2048)
		]) {
			expect(safeReturnPath(bad), String(bad)).toBe('/');
		}
	});
});

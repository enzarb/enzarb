import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const scss = readFileSync(new URL('./global.scss', import.meta.url), 'utf8');

function theme(mixin: string): Record<string, string> {
	const start = scss.indexOf(`@mixin ${mixin} {`);
	const end = scss.indexOf('\n}\n', start);
	const tokens: Record<string, string> = {};
	for (const [, name, value] of scss
		.slice(start, end)
		.matchAll(/--ez-([\w-]+):\s*(#[0-9a-f]{6});/gi))
		tokens[name] = value;
	return tokens;
}

function luminance(hex: string) {
	const [r, g, b] = [1, 3, 5].map((i) => {
		const c = parseInt(hex.slice(i, i + 2), 16) / 255;
		return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function ratio(a: string, b: string) {
	const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
	return (hi + 0.05) / (lo + 0.05);
}

const SURFACES = ['bg', 'surface', 'surface-sunken', 'surface-hover'];
const TEXT = ['text', 'text-muted', 'text-subtle', 'accent-text'];
const STATUS = ['success', 'warning', 'danger', 'info'];

describe.each([
	['light', 'theme-light'],
	['dark', 'theme-dark']
])('%s theme contrast (WCAG AA)', (_, mixin) => {
	const t = theme(mixin);

	it('defines the hex tokens', () => {
		expect(Object.keys(t).length).toBeGreaterThan(30);
	});

	it.each(TEXT.flatMap((fg) => SURFACES.map((bg) => [fg, bg])))(
		'%s on %s is >= 4.5:1',
		(fg, bg) => {
			expect(ratio(t[fg], t[bg])).toBeGreaterThanOrEqual(4.5);
		}
	);

	it('accent text is readable on the soft accent tint', () => {
		expect(ratio(t['accent-text'], t['accent-soft'])).toBeGreaterThanOrEqual(4.5);
	});

	it.each(['accent', 'accent-hover'])('accent-fg on %s is >= 4.5:1', (bg) => {
		expect(ratio(t['accent-fg'], t[bg])).toBeGreaterThanOrEqual(4.5);
	});

	it.each(['accent', 'control-border'])('%s is >= 3:1 against bg and surface', (fg) => {
		expect(ratio(t[fg], t['bg'])).toBeGreaterThanOrEqual(3);
		expect(ratio(t[fg], t['surface'])).toBeGreaterThanOrEqual(3);
	});

	it.each(STATUS)('%s text is >= 4.5:1 on its tint and the surface', (name) => {
		expect(ratio(t[`${name}-fg`], t[`${name}-bg`])).toBeGreaterThanOrEqual(4.5);
		expect(ratio(t[`${name}-fg`], t['surface'])).toBeGreaterThanOrEqual(4.5);
	});

	it('danger-solid-fg on danger-solid is >= 4.5:1', () => {
		expect(ratio(t['danger-solid-fg'], t['danger-solid'])).toBeGreaterThanOrEqual(4.5);
	});

	it('logo wordmark is readable on the page background', () => {
		expect(ratio(t['logo-ink'], t['bg'])).toBeGreaterThanOrEqual(4.5);
		expect(ratio(t['logo-dev'], t['bg'])).toBeGreaterThanOrEqual(4.5);
	});
});

describe('brand palette', () => {
	const dark = theme('theme-dark');
	it.each([
		['bg', '#0b0f17'],
		['surface', '#131b2b'],
		['border', '#232f46'],
		['accent', '#00f0ff'],
		['success-fg', '#10b981'],
		['text', '#f8fafc'],
		['text-muted', '#94a3b8']
	])('dark %s is the brand value', (name, hex) => {
		expect(dark[name]).toBe(hex);
	});
});

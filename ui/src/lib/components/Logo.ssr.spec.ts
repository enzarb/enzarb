import { readFileSync, readdirSync } from 'node:fs';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';
import Logo from './Logo.svelte';
import { markPaths } from '../../../scripts/mark-geometry.mjs';

describe('Logo', () => {
	it('links home with an accessible name, a 22px mark and the name', () => {
		const { body } = render(Logo);
		expect(body).toContain('aria-label="enzarb home"');
		expect(body).toContain('href="/"');
		expect(body).toContain('width="22"');
		expect(body).toMatch(/EN<span class="ez-logo__z">Z<\/span>ARB/);
	});

	it('swaps the text for the outlined wordmark', () => {
		const { body } = render(Logo, { props: { variant: 'wordmark', size: 32 } });
		expect(body).toContain('ez-logo__word');
		expect(body).not.toContain('<span');
		expect(body).toContain('width="32"');
		expect(body).toContain('fill="var(--ez-logo-ink)"');
	});

	it('inlines E, Z and overlap paths with theme fills', () => {
		const { body } = render(Logo);
		const { E, Z, overlap } = markPaths();
		for (const d of [E, Z, overlap]) expect(body).toContain(`d="${d}"`);
		for (const v of ['e', 'z', 'overlap']) expect(body).toContain(`fill="var(--ez-logo-${v})"`);
	});

	it('uses no filters, clip paths or id references, so instances never collide', () => {
		for (const variant of ['mark', 'wordmark'] as const) {
			const { body } = render(Logo, { props: { variant } });
			expect(body).not.toContain('<filter');
			expect(body).not.toContain('clip');
			expect(body).not.toMatch(/url\(#/);
			expect(body).not.toMatch(/\sid="/);
		}
	});
});

describe('brand assets', () => {
	const dir = new URL('../../../static/brand/', import.meta.url);
	const read = (name: string) => readFileSync(new URL(name, dir), 'utf8');
	const files = [
		'enzarb-lockup-on-dark.svg',
		'enzarb-lockup.svg',
		'enzarb-mark-on-dark.svg',
		'enzarb-mark.svg',
		'enzarb-wordmark-on-dark.svg',
		'enzarb-wordmark.svg'
	];

	it.each(files)('%s is a standalone svg with no filter, font or background', (name) => {
		const svg = read(name);
		expect(svg).toMatch(/^<svg [^>]*viewBox="/);
		expect(svg.trimEnd().endsWith('</svg>')).toBe(true);
		expect(svg).not.toMatch(/<filter|filter=|<text|<rect|<image/);
		for (const [, id] of svg.matchAll(/url\(#([^)]+)\)/g)) expect(svg).toContain(`id="${id}"`);
	});

	it.each([
		['enzarb-mark.svg', ['#8a94a6', '#ff4d1f', '#14161b']],
		['enzarb-mark-on-dark.svg', ['#8a96ad', '#ff4d1f', '#f2f2ee']]
	])('%s is the E, Z and overlap in its ground colours', (name, colours) => {
		const svg = read(name);
		const { E, Z, overlap } = markPaths();
		expect(svg.match(/<path /g)).toHaveLength(3);
		[E, Z, overlap].forEach((d, i) => expect(svg).toContain(`fill="${colours[i]}" d="${d}"`));
	});

	it.each(['enzarb-wordmark.svg', 'enzarb-lockup.svg'])(
		'%s outlines ENZARB with an orange Z',
		(name) => {
			expect(read(name)).toContain('fill="#ff4d1f"');
			expect(read(name)).toContain('fill="#14161b"');
		}
	);

	it('Logo.svelte inlines the generated wordmark outlines', () => {
		const word = read('enzarb-wordmark.svg');
		const { body } = render(Logo, { props: { variant: 'wordmark' } });
		for (const [, d] of word.matchAll(/ d="([^"]+)"/g)) expect(body).toContain(d);
	});

	it('lists every brand file', () => {
		expect(readdirSync(dir).sort()).toEqual(files);
	});
});

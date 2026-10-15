import { readFileSync, readdirSync } from 'node:fs';
import { render } from 'svelte/server';
import { describe, expect, it } from 'vitest';
import Logo from './Logo.svelte';

describe('Logo', () => {
	it('links home with an accessible name and a 22px mark', () => {
		const { body } = render(Logo);
		expect(body).toContain('aria-label="enzarb home"');
		expect(body).toContain('href="/"');
		expect(body).toContain('width="22"');
		expect(body).toContain('<span>enzarb</span>');
	});

	it('swaps the text for the wordmark', () => {
		const { body } = render(Logo, { props: { variant: 'wordmark', size: 32 } });
		expect(body).toContain('ez-logo__word');
		expect(body).not.toContain('<span>');
		expect(body).toContain('width="32"');
	});

	it('uses no filters or id references, so instances never collide', () => {
		const { body } = render(Logo, { props: { variant: 'wordmark' } });
		expect(body).not.toContain('<filter');
		expect(body).not.toMatch(/url\(#/);
		expect(body).not.toMatch(/\sid="/);
	});
});

describe('brand assets', () => {
	const dir = new URL('../../../static/brand/', import.meta.url);
	const read = (name: string) => readFileSync(new URL(name, dir), 'utf8');

	it.each([
		'enzarb-mark.svg',
		'enzarb-lockup.svg',
		'enzarb-wordmark.svg',
		'enzarb-wordmark-on-light.svg',
		'enzarb-mark-flat.svg'
	])('%s is a standalone svg with no filter and no background', (name) => {
		const svg = read(name);
		expect(svg).toMatch(/^<svg [^>]*viewBox="/);
		expect(svg.trimEnd().endsWith('</svg>')).toBe(true);
		expect(svg).not.toContain('<filter');
		expect(svg).not.toContain('filter=');
		expect(svg).not.toContain('bgCanvas');
		// Every referenced gradient is defined in the same file.
		for (const [, id] of svg.matchAll(/url\(#([^)]+)\)/g)) expect(svg).toContain(`id="${id}"`);
	});

	it('the flat mark is two flat facets on an obsidian squircle', () => {
		const svg = read('enzarb-mark-flat.svg');
		expect(svg).toContain('fill="#0b0f17"');
		expect(svg.match(/<polygon /g)).toHaveLength(2);
		expect(svg).not.toMatch(/Gradient|stroke/);
	});

	it('ships the sheet unchanged, filter included', () => {
		const sheet = read('enzarb-sheet.svg');
		expect(sheet).toContain('viewBox="0 0 1200 900"');
		expect(sheet).toContain('<g id="enzarb-emblem" filter="url(#dropShadow)">');
	});

	it('lists every brand file', () => {
		expect(readdirSync(dir).sort()).toEqual([
			'enzarb-lockup.svg',
			'enzarb-mark-flat.svg',
			'enzarb-mark.svg',
			'enzarb-sheet.svg',
			'enzarb-wordmark-on-light.svg',
			'enzarb-wordmark.svg'
		]);
	});
});

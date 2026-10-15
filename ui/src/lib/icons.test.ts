import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { Resvg } from '@resvg/resvg-js';
import { beforeAll, describe, expect, it } from 'vitest';

const root = new URL('../../', import.meta.url);
const read = (path: string) => readFileSync(new URL(path, root));

function pngSize(buf: Buffer) {
	expect(buf.subarray(1, 4).toString()).toBe('PNG');
	return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

describe('generated icons', () => {
	beforeAll(() => {
		// Regenerate so the test also proves the script runs; outputs are deterministic.
		execFileSync('node', ['scripts/generate-icons.mjs'], { cwd: root });
	});

	it.each([
		['apple-touch-icon.png', 180],
		['icon-192.png', 192],
		['icon-maskable-192.png', 192],
		['icon-512.png', 512],
		['icon-maskable-512.png', 512],
		['badge.png', 96]
	])('%s is %i px square', (file, size) => {
		expect(pngSize(read(`static/${file}`))).toEqual({ width: size, height: size });
	});

	it('favicon.ico holds 16, 32 and 48 px images', () => {
		const ico = read('static/favicon.ico');
		expect(ico.readUInt16LE(0)).toBe(0);
		expect(ico.readUInt16LE(2)).toBe(1);
		const count = ico.readUInt16LE(4);
		const sizes = Array.from({ length: count }, (_, i) => ico.readUInt8(6 + i * 16) || 256);
		expect(sizes.sort((a, b) => a - b)).toEqual([16, 32, 48]);
	});

	it('keeps the maskable glyph inside the 80% safe zone', () => {
		const svg = readFileSync(new URL('static/brand/enzarb-mark-flat.svg', root), 'utf8');
		// Drop the squircle so only the glyph has alpha, then apply the maskable transform
		// from generate-icons.mjs (central 70%).
		const body = svg.replace(/<rect[^>]*\/>/, '').replace(/<\/?svg[^>]*>/g, '');
		const glyph = `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><g transform="translate(9.6 9.6) scale(0.7)">${body}</g></svg>`;
		const img = new Resvg(glyph, { fitTo: { mode: 'width', value: 640 } }).render();
		const { width, height, pixels } = img;
		let min = width;
		let max = 0;
		let minY = height;
		let maxY = 0;
		for (let y = 0; y < height; y++)
			for (let x = 0; x < width; x++)
				if (pixels[(y * width + x) * 4 + 3] > 0) {
					min = Math.min(min, x);
					max = Math.max(max, x);
					minY = Math.min(minY, y);
					maxY = Math.max(maxY, y);
				}
		const margin = width * 0.1;
		expect(min).toBeGreaterThanOrEqual(margin);
		expect(minY).toBeGreaterThanOrEqual(margin);
		expect(max).toBeLessThanOrEqual(width - margin);
		expect(maxY).toBeLessThanOrEqual(height - margin);
	});

	it('lists only existing icons in the manifest', () => {
		const manifest = JSON.parse(read('static/manifest.webmanifest').toString());
		expect(manifest.display).toBe('standalone');
		expect(manifest.icons.map((i: { purpose: string }) => i.purpose).sort()).toEqual([
			'any',
			'any',
			'maskable',
			'maskable'
		]);
		for (const icon of manifest.icons) {
			expect(existsSync(new URL(`static${icon.src}`, root))).toBe(true);
		}
	});
});

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import {
	SAFE_RADIUS,
	anySvg,
	badgeSvg,
	faviconSvg,
	maskableSvg,
	render
} from '../../scripts/icon-lib.mjs';
import { COLORS } from '../../scripts/mark-geometry.mjs';

const root = new URL('../../', import.meta.url);
const read = (path: string) => readFileSync(new URL(path, root));

function pngSize(buf: Buffer) {
	expect(buf.subarray(1, 4).toString()).toBe('PNG');
	return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
}

/** Bounding box of the drawn (alpha > 0) pixels. */
function drawnBox({ width, height, pixels }: { width: number; height: number; pixels: Buffer }) {
	let x0 = width,
		x1 = 0,
		y0 = height,
		y1 = 0;
	for (let y = 0; y < height; y++)
		for (let x = 0; x < width; x++)
			if (pixels[(y * width + x) * 4 + 3] > 0) {
				x0 = Math.min(x0, x);
				x1 = Math.max(x1, x);
				y0 = Math.min(y0, y);
				y1 = Math.max(y1, y);
			}
	return { x0, x1, y0, y1 };
}

describe('generated icons', () => {
	beforeAll(() => {
		// Regenerate so the test also proves the script runs; outputs are deterministic.
		execFileSync('node', ['scripts/generate-icons.mjs'], { cwd: root });
	});

	it.each([
		['apple-touch-icon-ez.png', 180],
		['icon-ez-192.png', 192],
		['icon-ez-maskable-192.png', 192],
		['icon-ez-512.png', 512],
		['icon-ez-maskable-512.png', 512],
		['badge-ez.png', 96]
	])('%s is %i px square', (file, size) => {
		expect(pngSize(read(`static/${file}`))).toEqual({ width: size, height: size });
	});

	it('favicon-ez.ico holds 16, 32 and 48 px images', () => {
		const ico = read('static/favicon-ez.ico');
		expect(ico.readUInt16LE(0)).toBe(0);
		expect(ico.readUInt16LE(2)).toBe(1);
		const count = ico.readUInt16LE(4);
		const sizes = Array.from({ length: count }, (_, i) => ico.readUInt8(6 + i * 16) || 256);
		expect(sizes.sort((a, b) => a - b)).toEqual([16, 32, 48]);
	});

	it('keeps the maskable mark inside the 80% safe circle', () => {
		const size = 640;
		const { width, pixels } = render(maskableSvg(true), size);
		const limit = size * SAFE_RADIUS;
		let drawn = 0;
		for (let i = 0; i < pixels.length; i += 4) {
			if (pixels[i + 3] === 0) continue;
			drawn++;
			const x = ((i / 4) % width) + 0.5 - size / 2;
			const y = Math.floor(i / 4 / width) + 0.5 - size / 2;
			// A pixel centre may sit up to half a pixel outside the true edge.
			expect(Math.hypot(x, y)).toBeLessThanOrEqual(limit + 0.75);
		}
		expect(drawn).toBeGreaterThan(0);
	});

	it('fills about 80% of the any icon', () => {
		const img = render(anySvg(true), 640);
		const { x0, x1, y0, y1 } = drawnBox(img);
		const longer = Math.max(x1 - x0 + 1, y1 - y0 + 1);
		expect(longer / img.width).toBeGreaterThan(0.78);
		expect(longer / img.width).toBeLessThan(0.82);
	});

	it('puts installable icons on a white ground with the light-ground colours', () => {
		for (const svg of [anySvg(), maskableSvg()]) {
			expect(svg).toContain('fill="#ffffff"');
			for (const c of Object.values(COLORS.light)) expect(svg).toContain(`fill="${c}"`);
		}
		const { pixels } = render(maskableSvg(), 64);
		expect([...pixels.subarray(0, 4)]).toEqual([255, 255, 255, 255]);
	});

	it('draws badge-ez.png in one colour', () => {
		const { pixels } = render(badgeSvg(), 96);
		let solid = 0;
		for (let i = 0; i < pixels.length; i += 4) {
			if (pixels[i + 3] === 0) continue;
			solid++;
			// resvg returns premultiplied RGBA, so white at any alpha has every channel equal to it.
			for (const c of [0, 1, 2]) expect(pixels[i + c]).toBe(pixels[i + 3]);
		}
		expect(solid).toBeGreaterThan(0);
		expect(badgeSvg()).not.toMatch(/fill="#(?!ffffff)/i);
	});

	it('favicon-ez.svg follows the system theme on a transparent ground', () => {
		const svg = read('static/favicon-ez.svg').toString();
		expect(svg).toBe(faviconSvg());
		expect(svg).toContain('@media (prefers-color-scheme: dark)');
		for (const c of [...Object.values(COLORS.light), ...Object.values(COLORS.dark)])
			expect(svg).toContain(c);
		expect(svg).not.toMatch(/<rect/);
		expect(read('src/lib/assets/favicon-ez.svg').toString()).toBe(svg);
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

	it('renames every icon so caches and installed apps refresh', () => {
		const old = [
			'favicon.svg',
			'favicon.ico',
			'apple-touch-icon.png',
			'icon-192.png',
			'icon-512.png',
			'icon-maskable-192.png',
			'icon-maskable-512.png',
			'badge.png'
		];
		const manifest = JSON.parse(read('static/manifest.webmanifest').toString());
		const html = read('src/app.html').toString();
		const sw = read('src/lib/sw-handlers.ts').toString();
		for (const icon of manifest.icons) expect(old).not.toContain(icon.src.replace(/^\//, ''));
		for (const name of old) {
			expect(existsSync(new URL(`static/${name}`, root))).toBe(false);
			expect(html).not.toContain(`"/${name}"`);
			expect(sw).not.toContain(`'/${name}'`);
		}
	});
});

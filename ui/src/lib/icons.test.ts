import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import {
	SAFE_RADIUS,
	anySvg,
	badgeSvg,
	glyphSvg,
	loadMark,
	maskableSvg,
	render
} from '../../scripts/icon-lib.mjs';

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

	it.each(['flat', 'tiny'])('keeps the %s maskable glyph inside the 80%% safe circle', (name) => {
		// Same transform as generate-icons.mjs, glyph only so just the mark has alpha.
		const size = 640;
		const { width, pixels } = render(maskableSvg(loadMark(name), true), size);
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
		const { width, height, pixels } = render(anySvg(loadMark('flat'), true), 640);
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
		const longer = Math.max(max - min, maxY - minY + 1);
		expect(longer / width).toBeGreaterThan(0.78);
		expect(longer / width).toBeLessThan(0.82);
	});

	describe('merged marks', () => {
		// Source coordinates (viewBox 250 46 700 700) along the edges the old separate shapes shared.
		const seams = [
			...[400, 420, 440, 460].map((y) => [376, y]), // E bar meets the E face
			...[560, 600, 640].map((y) => [582, y]), // Z pillar meets the hook
			...[0.25, 0.5, 0.75].map((t) => [534 + 48 * t, 522 + 16 * t]) // Z diagonal meets the pillar
		];
		it('flat has no hairline seams', () => {
			const size = 1400;
			const { width, pixels } = render(glyphSvg(loadMark('flat')), size);
			for (const [x, y] of seams) {
				const px = Math.round(((x - 250) / 700) * size);
				const py = Math.round(((y - 46) / 700) * size);
				for (const dx of [-1, 0, 1])
					expect(pixels[(py * width + px + dx) * 4 + 3], `seam at ${x},${y}`).toBe(255);
			}
		});
	});

	describe('tiny mark at 16px', () => {
		const img = render(glyphSvg(loadMark('tiny')), 16);
		// Alpha of the pixel holding a source-coordinate point (viewBox 250 46 700 700).
		const alpha = (x: number, y: number) => {
			const px = Math.floor(((x - 250) * 16) / 700);
			const py = Math.floor(((y - 46) * 16) / 700);
			return img.pixels[(py * 16 + px) * 4 + 3];
		};

		it.each([
			['hexagon left side', 377, 420],
			['hexagon apex', 600, 115],
			['hexagon upper left', 480, 200],
			['E middle bar', 447, 418],
			['Z diagonal', 730, 326],
			['Z stem', 613, 600]
		])('keeps the %s visible', (_, x, y) => {
			expect(alpha(x, y)).toBeGreaterThanOrEqual(200);
		});

		it('keeps the gap between the E and the Z open', () => {
			expect(alpha(530, 610)).toBe(0);
			expect(alpha(530, 640)).toBe(0);
		});
	});

	it('draws badge.png in one colour', () => {
		const { pixels } = render(badgeSvg(loadMark('tiny')), 96);
		let solid = 0;
		for (let i = 0; i < pixels.length; i += 4) {
			if (pixels[i + 3] === 0) continue;
			solid++;
			// resvg returns premultiplied RGBA, so white at any alpha has every channel equal to it.
			for (const c of [0, 1, 2]) expect(pixels[i + c]).toBe(pixels[i + 3]);
		}
		expect(solid).toBeGreaterThan(0);
		expect(badgeSvg(loadMark('tiny'))).not.toMatch(/fill="#(?!ffffff)/i);
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

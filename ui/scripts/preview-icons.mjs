// Renders the brand mark at 16, 32, 48, 180 and 512px on light and dark grounds into one PNG
// (docs/brand-preview.png), with the 16px renders enlarged without smoothing. Run: npm run icons:preview
import { writeFileSync } from 'node:fs';
import { png, render } from './icon-lib.mjs';
import { COLORS, markBody } from './mark-geometry.mjs';

const SIZES = [16, 32, 48, 180, 512];
const ZOOM = 8;
const PAD = 24;

/** @param {'light' | 'dark'} g @param {number} x @param {number} y @param {number} size */
const nested = (g, x, y, size) =>
	`<svg x="${x}" y="${y}" width="${size}" height="${size}" viewBox="8 8 48 48">${markBody(COLORS[g])}</svg>`;

const rowH = 512 + 2 * PAD;
const zoomed = 16 * ZOOM;
const width = PAD + SIZES.reduce((w, s) => w + s + PAD, 0) + zoomed + PAD;
const rows = /** @type {['light' | 'dark', string][]} */ ([
	['light', '#f4f5f7'],
	['dark', '#0d0f14']
]);

let body = '';
rows.forEach(([g, fill], r) => {
	const y0 = r * rowH;
	body += `<rect x="0" y="${y0}" width="${width}" height="${rowH}" fill="${fill}" />`;
	let x = PAD;
	for (const size of SIZES) {
		body += nested(g, x, y0 + PAD, size);
		x += size + PAD;
	}
	// 16px rendered for real, then scaled up with nearest-neighbour to show the pixels.
	const data = png(
		`<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="${fill}" />${nested(g, 0, 0, 16)}</svg>`,
		16
	);
	body += `<image x="${x}" y="${y0 + PAD}" width="${zoomed}" height="${zoomed}" image-rendering="optimizeSpeed" href="data:image/png;base64,${data.toString('base64')}" />`;
});

const sheet = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${rowH * 2}">${body}</svg>`;
const out = new URL('../../docs/brand-preview.png', import.meta.url);
writeFileSync(out, render(sheet, width).asPng());
console.log(`wrote ${out.pathname}`);

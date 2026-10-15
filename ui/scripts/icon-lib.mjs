// Shared by generate-icons.mjs, preview-icons.mjs and the icon tests: reads the brand marks and
// places their glyph on the 64x64 canvases the icon set is drawn on.
import { readFileSync } from 'node:fs';
import { Resvg } from '@resvg/resvg-js';

export const BG = '#0b0f17';
export const CANVAS = 64;
/** Fraction of the canvas the glyph's longer side fills on `any` and touch icons. */
export const FILL = 0.8;
/** Maskable icons keep the whole glyph inside this radius (fraction of the side): the 80% safe circle. */
export const SAFE_RADIUS = 0.4;

/**
 * @typedef {{ x0: number, x1: number, y0: number, y1: number }} BBox
 * @typedef {{
 *   name: string,
 *   url: URL,
 *   svg: string,
 *   viewBox: number[],
 *   rect: string,
 *   glyph: string,
 *   points: number[][],
 *   bbox: BBox
 * }} Mark
 */

/** @param {string} name */
const markUrl = (name) => new URL(`../static/brand/enzarb-mark-${name}.svg`, import.meta.url);

/**
 * Reads `flat` or `tiny`: viewBox, squircle rect, glyph markup and the glyph's path vertices.
 * @param {string} name
 * @returns {Mark}
 */
export function loadMark(name) {
	const svg = readFileSync(markUrl(name), 'utf8');
	const viewBox = (svg.match(/viewBox="([^"]+)"/)?.[1] ?? '').split(/\s+/).map(Number);
	const body = svg.slice(svg.indexOf('>') + 1, svg.lastIndexOf('</svg>'));
	const rect = body.match(/<rect[^>]*\/>/)?.[0] ?? '';
	const glyph = body.replace(rect, '').trim();
	const points = [...glyph.matchAll(/ d="([^"]+)"/g)].flatMap(([, d]) =>
		[...d.matchAll(/(-?[\d.]+),(-?[\d.]+)/g)].map(([, x, y]) => [Number(x), Number(y)])
	);
	const xs = points.map((p) => p[0]);
	const ys = points.map((p) => p[1]);
	const bbox = {
		x0: Math.min(...xs),
		x1: Math.max(...xs),
		y0: Math.min(...ys),
		y1: Math.max(...ys)
	};
	return { name, url: markUrl(name), svg, viewBox, rect, glyph, points, bbox };
}

/**
 * `translate scale` that centres the glyph's bounding box on the canvas at scale `s`.
 * @param {Mark} mark
 * @param {number} s
 */
const place = (mark, s) => {
	const { x0, x1, y0, y1 } = mark.bbox;
	const tx = CANVAS / 2 - (s * (x0 + x1)) / 2;
	const ty = CANVAS / 2 - (s * (y0 + y1)) / 2;
	return `translate(${+tx.toFixed(4)} ${+ty.toFixed(4)}) scale(${+s.toFixed(6)})`;
};

/**
 * Glyph scaled so its longer side is `FILL` of the canvas.
 * @param {Mark} mark
 */
export const anyTransform = (mark) => {
	const { x0, x1, y0, y1 } = mark.bbox;
	return place(mark, (FILL * CANVAS) / Math.max(x1 - x0, y1 - y0));
};

/**
 * Glyph scaled so its farthest vertex (a hexagon corner) sits on the safe circle.
 * @param {Mark} mark
 */
export function maskableTransform(mark) {
	const { x0, x1, y0, y1 } = mark.bbox;
	const cx = (x0 + x1) / 2;
	const cy = (y0 + y1) / 2;
	const far = Math.max(...mark.points.map(([x, y]) => Math.hypot(x - cx, y - cy)));
	// Round down so rounding the printed scale can never push a corner outside the circle.
	const s = Math.floor(((SAFE_RADIUS * CANVAS) / far) * 1e6) / 1e6;
	return place(mark, s);
}

/** @param {string} inner */
const wrap = (inner) =>
	`<svg xmlns="http://www.w3.org/2000/svg" width="${CANVAS}" height="${CANVAS}" viewBox="0 0 ${CANVAS} ${CANVAS}">${inner}</svg>`;
const bg = `<rect width="${CANVAS}" height="${CANVAS}" fill="${BG}" />`;

/**
 * Squircle icon with the glyph at 80% (or the glyph alone when `glyphOnly`, for tests).
 * @param {Mark} mark
 * @param {boolean} [glyphOnly]
 */
export const anySvg = (mark, glyphOnly = false) =>
	wrap(
		`${glyphOnly ? '' : `<rect width="${CANVAS}" height="${CANVAS}" rx="14" fill="${BG}" />`}<g transform="${anyTransform(mark)}">${mark.glyph}</g>`
	);
/**
 * Full-bleed background, glyph inside the safe circle (or alone when `glyphOnly`, for tests).
 * @param {Mark} mark
 * @param {boolean} [glyphOnly]
 */
export const maskableSvg = (mark, glyphOnly = false) =>
	wrap(`${glyphOnly ? '' : bg}<g transform="${maskableTransform(mark)}">${mark.glyph}</g>`);
/**
 * iOS rounds the corners itself, so the touch icon is full-bleed too.
 * @param {Mark} mark
 */
export const touchSvg = (mark) =>
	wrap(`${bg}<g transform="${anyTransform(mark)}">${mark.glyph}</g>`);
/**
 * Notification badges are alpha masks: one white silhouette on transparent, no background.
 * @param {Mark} mark
 */
export const badgeSvg = (mark) =>
	wrap(
		`<g transform="${anyTransform(mark)}">${mark.glyph.replace(/fill="#[0-9a-f]{6}"/gi, 'fill="#ffffff"')}</g>`
	);
/**
 * The glyph alone in the mark's own viewBox, transparent.
 * @param {Mark} mark
 */
export const glyphSvg = (mark) =>
	`<svg xmlns="http://www.w3.org/2000/svg" viewBox="${mark.viewBox.join(' ')}">${mark.glyph}</svg>`;

/**
 * @param {string} svg
 * @param {number} size
 */
export function render(svg, size) {
	return new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render();
}
/**
 * @param {string} svg
 * @param {number} size
 */
export const png = (svg, size) => render(svg, size).asPng();

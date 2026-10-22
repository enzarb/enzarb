// Shared by generate-icons.mjs, preview-icons.mjs and the icon tests: places the overlaid EZ mark
// (mark-geometry.mjs) on the 64x64 canvases the icon set is drawn on.
import { Resvg } from '@resvg/resvg-js';
import { COLORS, ezPaths, markBody, pathPoints } from './mark-geometry.mjs';

/** Installable icons sit on white with the light-ground colours. */
export const BG = '#ffffff';
export const CANVAS = 64;
/** Fraction of the canvas the mark's 48-unit box fills on `any` and touch icons. */
export const FILL = 0.8;
/** Maskable icons keep the whole mark inside this radius (fraction of the side): the 80% safe circle. */
export const SAFE_RADIUS = 0.4;

/** The mark's box in its own 64x64 viewBox. */
const BOX = { x0: 8, x1: 56, y0: 8, y1: 56 };
const POINTS = Object.values(ezPaths()).flatMap(pathPoints);

/** @param {number} s */
const place = (s) => {
	const t = CANVAS / 2 - (s * (BOX.x0 + BOX.x1)) / 2;
	return `translate(${+t.toFixed(4)} ${+t.toFixed(4)}) scale(${+s.toFixed(6)})`;
};

/** Mark scaled so its box is `FILL` of the canvas. */
export const anyTransform = () => place((FILL * CANVAS) / (BOX.x1 - BOX.x0));

/** Mark scaled so its farthest vertex (a box corner) sits on the safe circle. */
export function maskableTransform() {
	const c = (BOX.x0 + BOX.x1) / 2;
	const far = Math.max(...POINTS.map(([x, y]) => Math.hypot(x - c, y - c)));
	// Round down so rounding the printed scale can never push a corner outside the circle.
	return place(Math.floor(((SAFE_RADIUS * CANVAS) / far) * 1e6) / 1e6);
}

/** @param {string} inner */
const wrap = (inner) =>
	`<svg xmlns="http://www.w3.org/2000/svg" width="${CANVAS}" height="${CANVAS}" viewBox="0 0 ${CANVAS} ${CANVAS}">${inner}</svg>`;
const bg = `<rect width="${CANVAS}" height="${CANVAS}" fill="${BG}" />`;
const light = markBody(COLORS.light);

/**
 * Rounded white tile with the mark at 80% (or the mark alone when `markOnly`, for tests).
 * @param {boolean} [markOnly]
 */
export const anySvg = (markOnly = false) =>
	wrap(
		`${markOnly ? '' : `<rect width="${CANVAS}" height="${CANVAS}" rx="14" fill="${BG}" />`}<g transform="${anyTransform()}">${light}</g>`
	);
/**
 * Full-bleed white, mark inside the safe circle (or alone when `markOnly`, for tests).
 * @param {boolean} [markOnly]
 */
export const maskableSvg = (markOnly = false) =>
	wrap(`${markOnly ? '' : bg}<g transform="${maskableTransform()}">${light}</g>`);
/** iOS rounds the corners itself, so the touch icon is full-bleed too. */
export const touchSvg = () => wrap(`${bg}<g transform="${anyTransform()}">${light}</g>`);
/** Notification badges are alpha masks: one white silhouette (E and Z) on transparent. */
export const badgeSvg = () =>
	wrap(
		`<g transform="${anyTransform()}">${markBody({ e: '#ffffff', z: '#ffffff', overlap: '#ffffff' })}</g>`
	);
/** The mark alone on transparent, light-ground colours (favicon.ico frames, preview). */
export const markSvg = () => wrap(`<g transform="${anyTransform()}">${light}</g>`);

/** favicon-ez.svg: transparent, colours follow the system theme. */
export const faviconSvg = () =>
	`<svg xmlns="http://www.w3.org/2000/svg" viewBox="8 8 48 48"><style>.e{fill:${COLORS.light.e}}.z{fill:${COLORS.light.z}}.o{fill:${COLORS.light.overlap}}@media (prefers-color-scheme: dark){.e{fill:${COLORS.dark.e}}.z{fill:${COLORS.dark.z}}.o{fill:${COLORS.dark.overlap}}}</style>${markBody(
		{ e: '', z: '', overlap: '' }
	)
		.replace('fill=""', 'class="e"')
		.replace('fill=""', 'class="z"')
		.replace('fill=""', 'class="o"')}</svg>\n`;

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

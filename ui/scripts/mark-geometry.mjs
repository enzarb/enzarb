// The overlaid EZ mark: an E and a Z in the same 48x48 box (8..56 of a 64x64 viewBox), the Z
// drawn over the E and their overlap in a third colour. Shared by build-brand.mjs, the icon
// generator and the tests. Geometry is fixed (issue #358); do not redesign it here.
import polygonClipping from 'polygon-clipping';

/** The mark's own 48-unit box, tight (icons place it on a 64x64 canvas instead). */
export const MARK_VIEWBOX = '8 8 48 48';

/** Light-ground and dark-ground colours of the three fills. */
export const COLORS = {
	light: { e: '#8a94a6', z: '#ff4d1f', overlap: '#14161b' },
	dark: { e: '#8a96ad', z: '#ff4d1f', overlap: '#f2f2ee' }
};

/**
 * E and Z outlines. t = bar thickness; the Z diagonal's perpendicular thickness equals t.
 * @param {number} [t]
 */
export function ezPaths(t = 9) {
	const x0 = 8,
		x1 = 56,
		y0 = 8,
		y1 = 56,
		f = (/** @type {number} */ n) => +n.toFixed(2);
	let w = t * 2;
	for (let i = 0; i < 60; i++) {
		const dx = x1 - x0 - w,
			dy = y1 - t - y0;
		w = (t * Math.hypot(dx, dy)) / dy + (dx * t) / dy;
	}
	const outer = (/** @type {number} */ y) => x1 + (x0 + w - x1) * ((y - y0) / (y1 - t - y0));
	const Z = `M${x0} ${y0}H${x1}L${f(x0 + w)} ${y1 - t}H${x1}V${y1}H${x0}L${f(x1 - w)} ${y0 + t}H${x0}Z`;
	const ym = (y0 + y1) / 2,
		a = ym - t / 2,
		b = ym + t / 2;
	const E = `M${x0} ${y0}H${x1}L${f(outer(y0 + t))} ${y0 + t}H${x0 + t}V${f(a)}H${f(outer(a))}L${f(outer(b))} ${f(b)}H${x0 + t}V${y1 - t}H${x1}V${y1}H${x0}Z`;
	return { E, Z };
}

/**
 * Vertices of an absolute M/L/H/V/Z path (the only commands ezPaths emits).
 * @param {string} d
 * @returns {[number, number][]}
 */
export function pathPoints(d) {
	/** @type {[number, number][]} */
	const pts = [];
	let x = 0,
		y = 0;
	for (const [, cmd, args] of d.matchAll(/([MLHVZ])([^MLHVZ]*)/g)) {
		const n = args.trim()
			? args
					.trim()
					.split(/[\s,]+/)
					.map(Number)
			: [];
		if (cmd === 'M' || cmd === 'L') [x, y] = n;
		else if (cmd === 'H') x = n[0];
		else if (cmd === 'V') y = n[0];
		else continue;
		pts.push([x, y]);
	}
	return pts;
}

/** @param {number[][][][]} multi */
const toPath = (multi) =>
	multi
		.flatMap((poly) =>
			poly.map((ring) => {
				const pts = ring.slice(0, -1).map(([x, y]) => `${+x.toFixed(2)} ${+y.toFixed(2)}`);
				return `M${pts.join('L')}Z`;
			})
		)
		.join('');

/** E, Z and the overlap (Z ∩ E) as three plain path strings, no clip paths needed. */
export function markPaths() {
	const { E, Z } = ezPaths();
	const overlap = toPath(
		/** @type {number[][][][]} */ (polygonClipping.intersection([pathPoints(E)], [pathPoints(Z)]))
	);
	return { E, Z, overlap };
}

/**
 * The mark as SVG elements; `fills` are colours or CSS var() expressions.
 * @param {{ e: string, z: string, overlap: string }} fills
 */
export function markBody(fills) {
	const { E, Z, overlap } = markPaths();
	return `<path fill="${fills.e}" d="${E}"/><path fill="${fills.z}" d="${Z}"/><path fill="${fills.overlap}" d="${overlap}"/>`;
}

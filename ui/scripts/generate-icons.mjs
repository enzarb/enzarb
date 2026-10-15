// Renders the favicon and app icon set from static/brand/enzarb-mark-flat.svg. Run: npm run icons
import { copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import { Resvg } from '@resvg/resvg-js';
import pngToIco from 'png-to-ico';

const out = (name) => new URL(`../static/${name}`, import.meta.url);
const flatUrl = new URL('../static/brand/enzarb-mark-flat.svg', import.meta.url);
const flat = readFileSync(flatUrl, 'utf8');

const BG = '#0b0f17';
const bgRect = /<rect[^>]*\/>/;
const glyph = flat.replace(bgRect, '').replace(/<\/?svg[^>]*>/g, '');
const wrap = (inner) =>
	`<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64">${inner}</svg>`;

// Full-bleed obsidian with the glyph scaled into the central 70%, inside the maskable 80% safe zone.
const maskable = wrap(
	`<rect width="64" height="64" fill="${BG}" /><g transform="translate(9.6 9.6) scale(0.7)">${glyph}</g>`
);
// Notification badges are alpha masks: a white silhouette, no background.
const badge = wrap(glyph.replace(/fill="#[0-9a-f]{6}"/gi, 'fill="#ffffff"'));
// iOS rounds the corners itself, so the touch icon is full-bleed too.
const touch = wrap(`<rect width="64" height="64" fill="${BG}" />${glyph}`);

function png(svg, size) {
	return new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render().asPng();
}

// The flat mark is already an obsidian squircle, so it works on light and dark tabs without a media rule.
writeFileSync(out('favicon.svg'), flat);
copyFileSync(flatUrl, new URL('../src/lib/assets/favicon.svg', import.meta.url));

for (const [file, svg, size] of [
	['apple-touch-icon.png', touch, 180],
	['icon-192.png', flat, 192],
	['icon-512.png', flat, 512],
	['icon-maskable-192.png', maskable, 192],
	['icon-maskable-512.png', maskable, 512],
	['badge.png', badge, 96]
]) {
	writeFileSync(out(file), png(svg, size));
}

writeFileSync(out('favicon.ico'), await pngToIco([16, 32, 48].map((s) => png(flat, s))));

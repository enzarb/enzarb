// Renders the favicon and app icon set from static/brand/enzarb-mark-flat.svg (48px and up) and
// enzarb-mark-tiny.svg (16-32px, thicker strokes). Run: npm run icons
import { copyFileSync, writeFileSync } from 'node:fs';
import pngToIco from 'png-to-ico';
import { anySvg, badgeSvg, loadMark, maskableSvg, png, touchSvg } from './icon-lib.mjs';

const out = (name) => new URL(`../static/${name}`, import.meta.url);
const flat = loadMark('flat');
const tiny = loadMark('tiny');

// The tiny mark is already an obsidian squircle, so it works on light and dark tabs without a media rule.
copyFileSync(tiny.url, out('favicon.svg'));
copyFileSync(tiny.url, new URL('../src/lib/assets/favicon.svg', import.meta.url));

for (const [file, svg, size] of [
	['apple-touch-icon.png', touchSvg(flat), 180],
	['icon-192.png', anySvg(flat), 192],
	['icon-512.png', anySvg(flat), 512],
	['icon-maskable-192.png', maskableSvg(flat), 192],
	['icon-maskable-512.png', maskableSvg(flat), 512],
	['badge.png', badgeSvg(tiny), 96]
]) {
	writeFileSync(out(file), png(svg, size));
}

const frames = [
	[tiny, 16],
	[tiny, 32],
	[flat, 48]
];
writeFileSync(out('favicon.ico'), await pngToIco(frames.map(([m, s]) => png(m.svg, s))));

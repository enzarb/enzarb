// Renders the favicon and app icon set from the overlaid EZ mark (mark-geometry.mjs). Run: npm run icons
// File names carry `-ez` so browsers and installed apps fetch the new set instead of a cached old one.
import { writeFileSync } from 'node:fs';
import pngToIco from 'png-to-ico';
import { anySvg, badgeSvg, faviconSvg, markSvg, maskableSvg, png, touchSvg } from './icon-lib.mjs';

const out = (/** @type {string} */ name) => new URL(`../static/${name}`, import.meta.url);

writeFileSync(out('favicon-ez.svg'), faviconSvg());
writeFileSync(new URL('../src/lib/assets/favicon-ez.svg', import.meta.url), faviconSvg());

for (const [file, svg, size] of /** @type {[string, string, number][]} */ ([
	['apple-touch-icon-ez.png', touchSvg(), 180],
	['icon-ez-192.png', anySvg(), 192],
	['icon-ez-512.png', anySvg(), 512],
	['icon-ez-maskable-192.png', maskableSvg(), 192],
	['icon-ez-maskable-512.png', maskableSvg(), 512],
	['badge-ez.png', badgeSvg(), 96]
])) {
	writeFileSync(out(file), png(svg, size));
}

writeFileSync(out('favicon-ez.ico'), await pngToIco([16, 32, 48].map((s) => png(markSvg(), s))));

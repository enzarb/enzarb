// Renders the icon set in static/ from the SVG sources in assets/icons/. Run: npm run icons
import { readFileSync, writeFileSync } from 'node:fs';
import { Resvg } from '@resvg/resvg-js';
import pngToIco from 'png-to-ico';

const src = (name) => new URL(`../assets/icons/${name}`, import.meta.url);
const out = (name) => new URL(`../static/${name}`, import.meta.url);

function png(svgFile, size) {
	const svg = readFileSync(src(svgFile), 'utf8');
	return new Resvg(svg, { fitTo: { mode: 'width', value: size } }).render().asPng();
}

// Browser tabs follow the OS theme, not the app's data-theme, so the favicon carries its own dark rule.
const DARK_RULE = `
	<style>
		@media (prefers-color-scheme: dark) {
			rect { fill: #8a90ff; }
			path { stroke: #0c0d11; }
		}
	</style>`;

const mark = readFileSync(src('mark.svg'), 'utf8');
writeFileSync(out('favicon.svg'), mark.replace('\n\t<rect', `${DARK_RULE}\n\t<rect`));

for (const [file, svg, size] of [
	['apple-touch-icon.png', 'mark-maskable.svg', 180],
	['icon-192.png', 'mark.svg', 192],
	['icon-512.png', 'mark.svg', 512],
	['icon-maskable-192.png', 'mark-maskable.svg', 192],
	['icon-maskable-512.png', 'mark-maskable.svg', 512],
	['badge.png', 'badge.svg', 96]
]) {
	writeFileSync(out(file), png(svg, size));
}

writeFileSync(out('favicon.ico'), await pngToIco([16, 32, 48].map((s) => png('mark.svg', s))));

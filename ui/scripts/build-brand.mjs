// Derives the logo assets in static/brand/ from the maintainer's logo sheet. Run: npm run brand
// enzarb-sheet.svg is the source of truth and is never rewritten; enzarb-mark-flat.svg and enzarb-mark-tiny.svg are hand-authored.
import { readFileSync, writeFileSync } from 'node:fs';

const dir = (name) => new URL(`../static/brand/${name}`, import.meta.url);
const sheet = readFileSync(dir('enzarb-sheet.svg'), 'utf8');

function group(id) {
	const start = sheet.indexOf(`<g id="${id}"`);
	const end = sheet.indexOf('</g>', start) + '</g>'.length;
	if (start < 0 || end < start) throw new Error(`#${id} not found in the sheet`);
	// No drop-shadow: it needs a filter region and blurs at small sizes.
	return sheet
		.slice(start, end)
		.replace(/\sfilter="[^"]*"/, '')
		.replace(/\stransform="translate\(0, ?0\)"/, '');
}

function gradients(body) {
	const ids = new Set([...body.matchAll(/url\(#([^)]+)\)/g)].map((m) => m[1]));
	const blocks = [
		...sheet.matchAll(/<(linearGradient|radialGradient) id="([^"]+)"[\s\S]*?<\/\1>/g)
	];
	return blocks
		.filter((m) => ids.has(m[2]))
		.map((m) => m[0])
		.join('\n    ');
}

function svg(viewBox, body) {
	const defs = gradients(body);
	const defsBlock = defs ? `  <defs>\n    ${defs}\n  </defs>\n` : '';
	return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}">\n${defsBlock}  ${body}\n</svg>\n`;
}

const emblem = group('enzarb-emblem');
const wordmark = group('enzarb-wordmark');
// The sheet's wordmark is near-white on dark; the on-light variant swaps in ink and a darker cyan.
const onLight = wordmark.replaceAll('#f1f5f9', '#0b0f17').replaceAll('#00e5ff', '#0e7490');

const MARK_BOX = '330 64 541 664';
const WORDMARK_BOX = '280 750 650 74';
const LOCKUP_BOX = '280 64 650 762';

const files = {
	'enzarb-mark.svg': svg(MARK_BOX, emblem),
	'enzarb-wordmark.svg': svg(WORDMARK_BOX, wordmark),
	'enzarb-wordmark-on-light.svg': svg(WORDMARK_BOX, onLight),
	'enzarb-lockup.svg': svg(LOCKUP_BOX, `${emblem}\n  ${wordmark}`)
};
for (const [name, content] of Object.entries(files)) writeFileSync(dir(name), content);

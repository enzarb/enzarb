// Writes the placeholder PWA icons to static/icons/ (no dependencies). SWAP POINT: when the new
// logo lands (the logo issue), replace the files in static/icons/ with exports of it (same names and
// sizes: icon-192.png, icon-512.png, icon-maskable-512.png, badge-96.png) and delete this script.
// Run: node scripts/make-icons.mjs
import { mkdirSync, writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';

const ACCENT = [0x4f, 0x46, 0xe5]; // --ez-accent in src/lib/styles/global.scss
const WHITE = [0xff, 0xff, 0xff];

const crcTable = Array.from({ length: 256 }, (_, n) => {
	let c = n;
	for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
	return c >>> 0;
});
const crc = (buf) => {
	let c = 0xffffffff;
	for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
	return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
	const body = Buffer.concat([Buffer.from(type), data]);
	const out = Buffer.alloc(8 + data.length + 4);
	out.writeUInt32BE(data.length, 0);
	body.copy(out, 4);
	out.writeUInt32BE(crc(body), 8 + data.length);
	return out;
};

function png(size, pixel) {
	const raw = Buffer.alloc(size * (size * 4 + 1));
	for (let y = 0; y < size; y++) {
		raw[y * (size * 4 + 1)] = 0;
		for (let x = 0; x < size; x++) {
			const [r, g, b, a] = pixel((x + 0.5) / size, (y + 0.5) / size);
			raw.set([r, g, b, a], y * (size * 4 + 1) + 1 + x * 4);
		}
	}
	const head = Buffer.alloc(13);
	head.writeUInt32BE(size, 0);
	head.writeUInt32BE(size, 4);
	head.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA
	return Buffer.concat([
		Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
		chunk('IHDR', head),
		chunk('IDAT', deflateSync(raw)),
		chunk('IEND', Buffer.alloc(0))
	]);
}

// A white disc on the accent colour: `bleed` fills the whole square (maskable), otherwise the square
// has rounded corners.
const disc = (x, y, r) => (x - 0.5) ** 2 + (y - 0.5) ** 2 <= r * r;
const rounded = (x, y, r) => {
	const dx = Math.max(Math.abs(x - 0.5) - (0.5 - r), 0);
	const dy = Math.max(Math.abs(y - 0.5) - (0.5 - r), 0);
	return dx * dx + dy * dy <= r * r;
};
const icon = (bleed, radius) => (x, y) => {
	if (!bleed && !rounded(x, y, 0.18)) return [0, 0, 0, 0];
	return disc(x, y, radius) ? [...WHITE, 255] : [...ACCENT, 255];
};
// Badges are masked by the OS: white shape on transparent.
const badge = (x, y) => (disc(x, y, 0.4) ? [...WHITE, 255] : [0, 0, 0, 0]);

mkdirSync('static/icons', { recursive: true });
writeFileSync('static/icons/icon-192.png', png(192, icon(false, 0.28)));
writeFileSync('static/icons/icon-512.png', png(512, icon(false, 0.28)));
// Maskable: content stays inside the central 80% safe zone.
writeFileSync('static/icons/icon-maskable-512.png', png(512, icon(true, 0.25)));
writeFileSync('static/icons/badge-96.png', png(96, badge));

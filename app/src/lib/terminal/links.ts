// Decide whether a URL detected in terminal output is safe to open in the user's
// browser. We refuse loopback/private/link-local targets so a link printed by a
// workspace process can't make the browser hit the user's local network, cloud
// metadata endpoints, etc. (DNS-rebinding / SSRF-from-browser surface), and to
// avoid mixed-content navigations from the https app.

function ipv4ToInt(ip: string): number | null {
	const parts = ip.split('.');
	if (parts.length !== 4) return null;
	let n = 0;
	for (const p of parts) {
		if (!/^\d{1,3}$/.test(p)) return null;
		const o = Number(p);
		if (o > 255) return null;
		n = n * 256 + o;
	}
	return n >>> 0;
}

function inRange(n: number, cidr: string): boolean {
	const [base, bitsStr] = cidr.split('/');
	const baseInt = ipv4ToInt(base);
	if (baseInt === null) return false;
	const bits = Number(bitsStr);
	const mask = bits === 0 ? 0 : (0xffffffff << (32 - bits)) >>> 0;
	return (n & mask) === (baseInt & mask);
}

const PRIVATE_V4 = [
	'0.0.0.0/8',
	'10.0.0.0/8',
	'100.64.0.0/10', // CGNAT
	'127.0.0.0/8', // loopback
	'169.254.0.0/16', // link-local (incl. 169.254.169.254 metadata)
	'172.16.0.0/12',
	'192.0.0.0/24',
	'192.168.0.0/16'
];

// Hostname suffixes that resolve to local/private scopes by convention.
const PRIVATE_SUFFIXES = ['.localhost', '.local', '.internal', '.lan', '.home'];

export function isPrivateHost(hostname: string): boolean {
	let h = hostname.toLowerCase();
	// Strip IPv6 brackets.
	if (h.startsWith('[') && h.endsWith(']')) h = h.slice(1, -1);

	if (h === 'localhost' || h === '0.0.0.0') return true;
	if (PRIVATE_SUFFIXES.some((s) => h.endsWith(s))) return true;

	// IPv6 loopback / unique-local (fc00::/7) / link-local (fe80::/10).
	if (h === '::1' || h === '::') return true;
	if (/^f[cd][0-9a-f]{0,2}:/.test(h)) return true; // fc00::/7
	if (/^fe[89ab][0-9a-f]:/.test(h)) return true; // fe80::/10

	const v4 = ipv4ToInt(h);
	if (v4 !== null) return PRIVATE_V4.some((c) => inRange(v4, c));

	return false;
}

// Returns true if the URL is an http(s) link to a non-private host (safe to open).
export function isExternalHttpUrl(uri: string): boolean {
	let u: URL;
	try {
		u = new URL(uri);
	} catch {
		return false;
	}
	if (u.protocol !== 'http:' && u.protocol !== 'https:') return false;
	return !isPrivateHost(u.hostname);
}

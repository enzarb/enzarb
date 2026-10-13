/** Cookie holding the page to return to after the GitHub authorize round trip. */
export const OAUTH_RETURN_COOKIE = 'enzarb_oauth_return';

/**
 * Accepts only a same-origin relative path (`/projects?x=1`). Anything else (absolute or
 * protocol-relative URLs, backslashes, control characters, the login routes) becomes `/`.
 */
export function safeReturnPath(value: string | null | undefined): string {
	if (!value || value.length > 2048 || !value.startsWith('/') || value.startsWith('//')) return '/';
	// eslint-disable-next-line no-control-regex
	if (value.includes('\\') || /[\u0000-\u001f\u007f]/.test(value)) return '/';
	let parsed: URL;
	try {
		parsed = new URL(value, 'http://return.invalid');
	} catch {
		return '/';
	}
	if (parsed.origin !== 'http://return.invalid') return '/';
	if (parsed.pathname === '/login' || parsed.pathname.startsWith('/login/')) return '/';
	const out = parsed.pathname + parsed.search + parsed.hash;
	// Dot segments are normalised by URL (`/.//evil` -> `//evil`), so check the result again.
	if (out.startsWith('//') || out.includes('\\')) return '/';
	return out;
}

export const PROBE_PATHS = new Set(['/healthz', '/readyz']);

/**
 * Paths served without a session. The dev component gallery (`/dev/...`) is public in dev only;
 * in production it is not public and its routes answer 404 anyway.
 */
export const isPublicPath = (path: string, dev: boolean): boolean =>
	path === '/login' ||
	path.startsWith('/login/') ||
	PROBE_PATHS.has(path) ||
	path === '/offline' ||
	(dev && (path === '/dev' || path.startsWith('/dev/')));

import type { RequestHandler } from './$types';

/** Liveness: the process is up. Deliberately does not touch the database. */
export const GET: RequestHandler = () =>
	new Response('ok', { headers: { 'content-type': 'text/plain', 'cache-control': 'no-store' } });

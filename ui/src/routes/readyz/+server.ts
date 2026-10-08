import { getSql } from '#lib/server/db.ts';
import { isDatabaseReady } from '#lib/server/health.ts';
import type { RequestHandler } from './$types';

/** Readiness: the database answers `SELECT 1` within a short timeout. */
export const GET: RequestHandler = async () => {
	const ok = await isDatabaseReady(getSql());
	return new Response(ok ? 'ok' : 'database unavailable', {
		status: ok ? 200 : 503,
		headers: { 'content-type': 'text/plain', 'cache-control': 'no-store' }
	});
};

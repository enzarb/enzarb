import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { sql } from '$lib/db';

export const GET: RequestHandler = async () => {
	const rows = await sql<{ jti: string }[]>`SELECT jti FROM jwt_revocations WHERE expires_at > now()`;
	return json({ revoked: rows.map((r) => r.jti) });
};

import { sql } from './db';
import type { RequestEvent } from '@sveltejs/kit';

export interface Session {
	id: string;
	userId: string;
	email: string;
	isAdmin: boolean;
	orgs: { id: string; slug: string; role: string }[];
}

export async function getSession(event: RequestEvent): Promise<Session | null> {
	const sessionId = event.cookies.get('session');
	if (!sessionId) return null;

	const rows = await sql`
		SELECT s.id, s.data, s.expires_at,
		       u.id as user_id, u.email, u.is_admin
		FROM sessions s
		JOIN users u ON u.id = s.user_id
		WHERE s.id = ${sessionId}
		  AND s.expires_at > now()
	`;
	if (!rows.length) return null;

	const row = rows[0];
	const orgs = await sql`
		SELECT o.id, o.slug, om.role
		FROM org_members om
		JOIN organizations o ON o.id = om.org_id
		WHERE om.user_id = ${row.user_id}
	`;

	return {
		id: row.id,
		userId: row.user_id,
		email: row.email,
		isAdmin: row.is_admin,
		orgs: orgs.map((r) => ({ id: r.id, slug: r.slug, role: r.role }))
	};
}

export async function createSession(userId: string): Promise<string> {
	const rows = await sql`
		INSERT INTO sessions (user_id, data)
		VALUES (${userId}, '{}')
		RETURNING id
	`;
	return rows[0].id;
}

export async function destroySession(sessionId: string): Promise<void> {
	await sql`DELETE FROM sessions WHERE id = ${sessionId}`;
}

export async function upsertUser(sub: string, email: string): Promise<string> {
	const rows = await sql`
		INSERT INTO users (email, oidc_sub)
		VALUES (${email}, ${sub})
		ON CONFLICT (oidc_sub) DO UPDATE SET email = EXCLUDED.email
		RETURNING id
	`;
	return rows[0].id;
}

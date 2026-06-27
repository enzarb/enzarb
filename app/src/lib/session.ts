import { sql } from './db';
import type { RequestEvent } from '@sveltejs/kit';

export interface Session {
	id: string;
	userId: string;
	email: string;
	isAdmin: boolean;
	orgs: { id: string; slug: string; role: string; privileges: string[]; personal: boolean }[];
}

export async function getSession(event: RequestEvent): Promise<Session | null> {
	const sessionId = event.cookies.get('session');
	if (!sessionId) return null;

	const rows = await sql`
		SELECT s.id, s.data, s.expires_at,
		       u.id as user_id, u.email, u.is_admin, u.username
		FROM sessions s
		JOIN users u ON u.id = s.user_id
		WHERE s.id = ${sessionId}
		  AND s.expires_at > now()
	`;
	if (!rows.length) return null;

	const row = rows[0];
	const orgs = await sql`
		SELECT o.id, o.slug, om.role, COALESCE(r.privileges, '{}') AS privileges
		FROM org_members om
		JOIN organizations o ON o.id = om.org_id
		LEFT JOIN org_roles r ON r.org_id = om.org_id AND r.name = om.role
		WHERE om.user_id = ${row.user_id}
		  AND o.deleted_at IS NULL
	`;

	return {
		id: row.id,
		userId: row.user_id,
		email: row.email,
		isAdmin: row.is_admin,
		orgs: orgs.map((r) => ({
			id: r.id,
			slug: r.slug,
			role: r.role,
			privileges: (r.privileges as string[]) ?? [],
			// A personal org's slug is the owner's username; such orgs are
			// single-user and have no team roles/membership management.
			personal: !!row.username && r.slug === row.username
		}))
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

export type GithubUpsertResult =
	| { type: 'linked'; userId: string }
	| { type: 'created'; userId: string }
	| { type: 'conflict'; existingUserId: string };

// Upsert a user authenticated via GitHub. If the github_id is already known,
// returns 'linked'. If no existing account, creates one and returns 'created'.
// If an existing account with the same email exists (but no github_id yet),
// returns 'conflict' so the caller can prompt for explicit confirmation before linking.
export async function upsertGithubUser(
	githubId: string,
	email: string,
	_displayName: string
): Promise<GithubUpsertResult> {
	// Already linked by github_id
	const byGithubId = await sql<{ id: string }[]>`
		SELECT id FROM users WHERE github_id = ${githubId}
	`;
	if (byGithubId.length) {
		await sql`UPDATE users SET email = ${email} WHERE id = ${byGithubId[0].id}`;
		return { type: 'linked', userId: byGithubId[0].id };
	}

	// Existing account with same email — require explicit confirmation before linking
	const byEmail = await sql<{ id: string }[]>`
		SELECT id FROM users WHERE email = ${email}
	`;
	if (byEmail.length) {
		return { type: 'conflict', existingUserId: byEmail[0].id };
	}

	// New user — create with github_id, no oidc_sub
	const rows = await sql<{ id: string }[]>`
		INSERT INTO users (email, github_id) VALUES (${email}, ${githubId}) RETURNING id
	`;
	return { type: 'created', userId: rows[0].id };
}

// Complete a pending account link after the user has re-authenticated.
export async function completePendingLink(token: string, verifiedUserId: string): Promise<boolean> {
	const rows = await sql<{ id: string; github_id: string; existing_user_id: string }[]>`
		SELECT id, github_id, existing_user_id FROM pending_account_links
		WHERE token = ${token} AND expires_at > now()
	`;
	if (!rows.length) return false;
	const { id, github_id, existing_user_id } = rows[0];
	if (existing_user_id !== verifiedUserId) return false;
	await sql`UPDATE users SET github_id = ${github_id} WHERE id = ${existing_user_id}`;
	await sql`DELETE FROM pending_account_links WHERE id = ${id}`;
	return true;
}

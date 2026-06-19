import { sql } from './db';

// Create an org and add a user as admin. No auth check — call only from trusted server-side code.
export async function createOrgWithAdmin(
	slug: string,
	displayName: string,
	userId: string,
	tier: 'free' | 'pro' = 'free'
): Promise<string> {
	const rows = await sql`
		INSERT INTO organizations (slug, display_name, tier)
		VALUES (${slug}, ${displayName}, ${tier})
		RETURNING id
	`;
	const orgId: string = rows[0].id;
	await sql`
		INSERT INTO org_members (org_id, user_id, role)
		VALUES (${orgId}, ${userId}, 'admin')
		ON CONFLICT (org_id, user_id) DO NOTHING
	`;
	return orgId;
}

// Check whether a username/org slug is available.
export async function isUsernameAvailable(username: string): Promise<boolean> {
	const users = await sql`SELECT 1 FROM users WHERE username = ${username} LIMIT 1`;
	const orgs = await sql`SELECT 1 FROM organizations WHERE slug = ${username} LIMIT 1`;
	return users.length === 0 && orgs.length === 0;
}

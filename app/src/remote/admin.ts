import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import { sql } from '$lib/db';
import { z } from 'zod/v4';

function requireAdmin() {
	const { locals } = getRequestEvent();
	if (!locals.session?.isAdmin) error(403, 'Admin required');
	return locals.session;
}

export async function listOrgs() {
	requireAdmin();
	return sql`
		SELECT o.id, o.slug, o.display_name, o.tier, o.created_at,
		       COUNT(om.user_id) as member_count
		FROM organizations o
		LEFT JOIN org_members om ON om.org_id = o.id
		GROUP BY o.id
		ORDER BY o.created_at DESC
	`;
}

export async function listUsers() {
	requireAdmin();
	return sql`SELECT id, email, is_admin, created_at FROM users ORDER BY created_at DESC`;
}

const CreateOrgSchema = z.object({
	slug: z.string().regex(/^[a-z0-9-]+$/),
	displayName: z.string().min(1),
	tier: z.enum(['free', 'pro']).default('free')
});

export async function createOrg(input: z.infer<typeof CreateOrgSchema>) {
	requireAdmin();
	const parsed = CreateOrgSchema.parse(input);
	const rows = await sql`
		INSERT INTO organizations (slug, display_name, tier)
		VALUES (${parsed.slug}, ${parsed.displayName}, ${parsed.tier})
		RETURNING id
	`;
	return rows[0];
}

const SetTierSchema = z.object({ orgId: z.string(), tier: z.enum(['free', 'pro']) });

export async function setOrgTier(input: z.infer<typeof SetTierSchema>) {
	requireAdmin();
	const parsed = SetTierSchema.parse(input);
	await sql`UPDATE organizations SET tier = ${parsed.tier} WHERE id = ${parsed.orgId}`;
}

const InviteSchema = z.object({
	orgId: z.string(),
	email: z.string().email(),
	role: z.enum(['member', 'admin']).default('member')
});

export async function inviteMember(input: z.infer<typeof InviteSchema>) {
	requireAdmin();
	const parsed = InviteSchema.parse(input);
	const users = await sql`SELECT id FROM users WHERE email = ${parsed.email}`;
	if (!users.length) error(404, 'User not found — they must log in first');
	await sql`
		INSERT INTO org_members (org_id, user_id, role)
		VALUES (${parsed.orgId}, ${users[0].id}, ${parsed.role})
		ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role
	`;
}

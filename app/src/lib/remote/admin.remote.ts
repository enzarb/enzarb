import { query, form, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql } from '$lib/db';

function requireAdmin() {
	const { locals } = getRequestEvent();
	if (!locals.session?.isAdmin) error(403, 'Admin required');
	return locals.session!;
}

export const listOrgs = query(async () => {
	requireAdmin();
	return sql`
		SELECT o.id, o.slug, o.display_name, o.tier, o.created_at,
		       COUNT(om.user_id) as member_count
		FROM organizations o
		LEFT JOIN org_members om ON om.org_id = o.id
		GROUP BY o.id
		ORDER BY o.created_at DESC
	`;
});

export const listUsers = query(async () => {
	requireAdmin();
	return sql`SELECT id, email, is_admin, created_at FROM users ORDER BY created_at DESC`;
});

const CreateOrgSchema = z.object({
	slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/),
	displayName: z.string().min(1),
	tier: z.enum(['free', 'pro']).default('free')
});

export const createOrgAdmin = form(CreateOrgSchema, async ({ slug, displayName, tier }) => {
	requireAdmin();
	await sql`
		INSERT INTO organizations (slug, display_name, tier)
		VALUES (${slug}, ${displayName}, ${tier})
	`;
});

const SetTierSchema = z.object({ orgId: z.string(), tier: z.enum(['free', 'pro']) });

export const setOrgTier = command(SetTierSchema, async ({ orgId, tier }) => {
	requireAdmin();
	await sql`UPDATE organizations SET tier = ${tier} WHERE id = ${orgId}`;
});

const InviteSchema = z.object({
	orgId: z.string(),
	email: z.string().email(),
	role: z.enum(['member', 'admin']).default('member')
});

export const inviteMember = form(InviteSchema, async ({ orgId, email, role }) => {
	requireAdmin();
	const users = await sql`SELECT id FROM users WHERE email = ${email}`;
	if (!users.length) error(404, 'User not found — they must log in first');
	await sql`
		INSERT INTO org_members (org_id, user_id, role)
		VALUES (${orgId}, ${users[0].id}, ${role})
		ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role
	`;
});

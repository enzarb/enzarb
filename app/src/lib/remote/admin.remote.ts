import { query, form, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { getSettings, updateSettings } from '$lib/settings';
import { softDeleteOrganization, recoverOrganization } from '$lib/k8s';

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
		WHERE o.deleted_at IS NULL
		GROUP BY o.id
		ORDER BY o.created_at DESC
	`;
});

export const listDeletedOrgs = query(async () => {
	requireAdmin();
	return sql`
		SELECT id, slug, display_name, tier, deleted_at
		FROM organizations
		WHERE deleted_at IS NOT NULL
		ORDER BY deleted_at DESC
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
	await listOrgs().refresh();
});

const SetTierSchema = z.object({ orgId: z.string(), tier: z.enum(['free', 'pro']) });

export const setOrgTier = command(SetTierSchema, async ({ orgId, tier }) => {
	requireAdmin();
	await sql`UPDATE organizations SET tier = ${tier} WHERE id = ${orgId}`;
});

const OrgIdSchema = z.object({ orgId: z.string() });

// Soft-delete an org: mark it in the DB (hides it from members) and stamp the
// Organization CR + its Projects so the operator retains then purges them.
export const deleteOrg = command(OrgIdSchema, async ({ orgId }) => {
	requireAdmin();
	const { retentionDays } = await getSettings();
	await softDeleteOrganization(orgId, retentionDays);
	await sql`UPDATE organizations SET deleted_at = now() WHERE id = ${orgId}`;
});

export const recoverOrg = command(OrgIdSchema, async ({ orgId }) => {
	requireAdmin();
	await recoverOrganization(orgId);
	await sql`UPDATE organizations SET deleted_at = NULL WHERE id = ${orgId}`;
});

export const getAdminSettings = query(async () => {
	requireAdmin();
	return getSettings();
});

// Form fields arrive as strings; validate they're positive numbers (integers
// for the whole-unit fields) and store them verbatim. `z.coerce.number()` has
// an `unknown` input type that's incompatible with `form`, so we keep strings.
const posNum = (opts: { int?: boolean; min?: number } = {}) =>
	z.string().refine(
		(s) => {
			const n = Number(s);
			if (!Number.isFinite(n)) return false;
			if (opts.min !== undefined && n < opts.min) return false;
			if (opts.int && !Number.isInteger(n)) return false;
			return true;
		},
		{ message: 'must be a positive number' }
	);

const SettingsSchema = z.object({
	freeMaxPvcGi: posNum({ int: true, min: 1 }),
	retentionDays: posNum({ int: true, min: 1 }),
	cpuSecondsPerUnit: posNum({ min: 0 }),
	memGiBSecondsPerUnit: posNum({ min: 0 }),
	netIngressPerGib: posNum({ min: 0 }),
	netEgressPerGib: posNum({ min: 0 }),
	storageGiBSecondsPerUnit: posNum({ min: 0 }),
	freeCPUSeconds: posNum({ min: 0 }),
	freeMemGiBSeconds: posNum({ min: 0 })
});

export const updateAdminSettings = form(SettingsSchema, async (v) => {
	requireAdmin();
	await updateSettings({
		free_max_pvc_gi: v.freeMaxPvcGi,
		retention_days: v.retentionDays,
		pricing_cpu_seconds_per_unit: v.cpuSecondsPerUnit,
		pricing_mem_gib_seconds_per_unit: v.memGiBSecondsPerUnit,
		pricing_net_ingress_per_gib: v.netIngressPerGib,
		pricing_net_egress_per_gib: v.netEgressPerGib,
		pricing_storage_gib_seconds_per_unit: v.storageGiBSecondsPerUnit,
		pricing_free_cpu_seconds: v.freeCPUSeconds,
		pricing_free_mem_gib_seconds: v.freeMemGiBSeconds
	});
	// Single-flight refresh so the form re-renders with the saved values instead
	// of the stale cached settings (which would otherwise need a manual reload).
	await getAdminSettings().refresh();
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
	await listOrgs().refresh();
});

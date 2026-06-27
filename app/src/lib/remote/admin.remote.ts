import { query, form, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql, seedOrgRoles } from '$lib/db';
import { getSettings, updateSettings } from '$lib/settings';
import {
	softDeleteOrganization,
	recoverOrganization,
	listProjects,
	deleteProject,
	forceDeleteProject,
	purgeAfterOf
} from '$lib/k8s';
import { BUILTIN_ROLE_NAMES } from '$lib/privileges';

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

type AdminProject = {
	orgId: string;
	orgSlug: string;
	// Email of the owning user when the org is personal (a personal org's slug is
	// the user's username); null for team orgs.
	userEmail: string | null;
	slug: string;
	displayName: string;
	phase: string;
	purgeAfter: string | null;
	deleting: boolean;
	createdAt: string | null;
};

// Every project across all orgs, with its deletion state, for admin oversight.
// Projects are namespaced under their org, so we fan out per org; an org whose
// namespace isn't provisioned (or is mid-teardown) is simply skipped. Personal
// orgs (slug == a user's username) are attributed to that user's email.
export const listAllProjects = query(async (): Promise<AdminProject[]> => {
	requireAdmin();
	const orgs = await sql`
		SELECT o.id, o.slug, u.email AS user_email
		FROM organizations o
		LEFT JOIN users u ON u.username = o.slug
		ORDER BY o.slug
	`;
	const out: AdminProject[] = [];
	for (const org of orgs) {
		let projects: any[];
		try {
			projects = await listProjects(org.id);
		} catch {
			continue;
		}
		for (const p of projects) {
			out.push({
				orgId: org.id,
				orgSlug: org.slug,
				userEmail: org.user_email ?? null,
				slug: p.metadata?.name ?? '',
				displayName: p.spec?.displayName ?? p.metadata?.name ?? '',
				phase: p.status?.phase ?? '',
				purgeAfter: purgeAfterOf(p),
				deleting: !!p.metadata?.deletionTimestamp,
				createdAt: p.metadata?.creationTimestamp ?? null
			});
		}
	}
	return out.sort((a, b) => a.orgSlug.localeCompare(b.orgSlug) || a.slug.localeCompare(b.slug));
});

const ProjectRefSchema = z.object({ orgId: z.string(), slug: z.string() });

// Hard-delete a project now (no retention window). The operator runs its cleanup
// finalizer, so the CR sits in PendingDeletion until that completes.
export const adminDeleteProject = command(ProjectRefSchema, async ({ orgId, slug }) => {
	requireAdmin();
	await deleteProject(orgId, slug);
	await listAllProjects().refresh();
});

// Force-remove a project wedged in deletion by clearing its cleanup finalizer.
export const adminForceDeleteProject = command(ProjectRefSchema, async ({ orgId, slug }) => {
	requireAdmin();
	await forceDeleteProject(orgId, slug);
	await listAllProjects().refresh();
});

const CreateOrgSchema = z.object({
	slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/),
	displayName: z.string().min(1),
	tier: z.enum(['free', 'pro']).default('free')
});

export const createOrgAdmin = form(CreateOrgSchema, async ({ slug, displayName, tier }) => {
	requireAdmin();
	const rows = await sql`
		INSERT INTO organizations (slug, display_name, tier)
		VALUES (${slug}, ${displayName}, ${tier})
		RETURNING id
	`;
	await seedOrgRoles(rows[0].id);
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
	// Invalidate all active sessions for org members so cached session data
	// for this org is immediately cleared rather than lingering for up to 7 days.
	await sql`
		DELETE FROM sessions
		WHERE user_id IN (SELECT user_id FROM org_members WHERE org_id = ${orgId})
	`;
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
	storageGiBSecondsPerUnit: posNum({ min: 0 }),
	zotStorageGiBSecondsPerUnit: posNum({ min: 0 }),
	netIngressInternalPerGib: posNum({ min: 0 }),
	netEgressInternalPerGib: posNum({ min: 0 }),
	netIngressExternalPerGib: posNum({ min: 0 }),
	netEgressExternalPerGib: posNum({ min: 0 }),
	freeCPUSeconds: posNum({ min: 0 }),
	freeMemGiBSeconds: posNum({ min: 0 }),
	freeStorageGiBSeconds: posNum({ min: 0 }),
	freeZotStorageGiBSeconds: posNum({ min: 0 }),
	freeNetIngressInternalGib: posNum({ min: 0 }),
	freeNetEgressInternalGib: posNum({ min: 0 }),
	freeNetIngressExternalGib: posNum({ min: 0 }),
	freeNetEgressExternalGib: posNum({ min: 0 })
});

export const updateAdminSettings = form(SettingsSchema, async (v) => {
	requireAdmin();
	await updateSettings({
		free_max_pvc_gi: v.freeMaxPvcGi,
		retention_days: v.retentionDays,
		pricing_cpu_seconds_per_unit: v.cpuSecondsPerUnit,
		pricing_mem_gib_seconds_per_unit: v.memGiBSecondsPerUnit,
		pricing_storage_gib_seconds_per_unit: v.storageGiBSecondsPerUnit,
		pricing_zot_storage_gib_seconds_per_unit: v.zotStorageGiBSecondsPerUnit,
		pricing_net_ingress_internal_per_gib: v.netIngressInternalPerGib,
		pricing_net_egress_internal_per_gib: v.netEgressInternalPerGib,
		pricing_net_ingress_external_per_gib: v.netIngressExternalPerGib,
		pricing_net_egress_external_per_gib: v.netEgressExternalPerGib,
		pricing_free_cpu_seconds: v.freeCPUSeconds,
		pricing_free_mem_gib_seconds: v.freeMemGiBSeconds,
		pricing_free_storage_gib_seconds: v.freeStorageGiBSeconds,
		pricing_free_zot_storage_gib_seconds: v.freeZotStorageGiBSeconds,
		pricing_free_net_ingress_internal_gib: v.freeNetIngressInternalGib,
		pricing_free_net_egress_internal_gib: v.freeNetEgressInternalGib,
		pricing_free_net_ingress_external_gib: v.freeNetIngressExternalGib,
		pricing_free_net_egress_external_gib: v.freeNetEgressExternalGib
	});
	// Single-flight refresh so the form re-renders with the saved values instead
	// of the stale cached settings (which would otherwise need a manual reload).
	await getAdminSettings().refresh();
});

const InviteSchema = z.object({
	orgId: z.string(),
	email: z.string().email(),
	role: z.enum(BUILTIN_ROLE_NAMES as [string, ...string[]]).default('member')
});

export const inviteMember = form(InviteSchema, async ({ orgId, email, role }) => {
	requireAdmin();
	const users = await sql`SELECT id FROM users WHERE email = ${email}`;
	if (!users.length) error(400, 'No account found for that email. Ask them to log in to Enzarb first.');
	await sql`
		INSERT INTO org_members (org_id, user_id, role)
		VALUES (${orgId}, ${users[0].id}, ${role})
		ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role
	`;
	await listOrgs().refresh();
});


export const listErrorLogs = query(async () => {
	requireAdmin();
	return sql<{ id: string; scope: string; level: string; message: string; stack: string | null; context: Record<string, unknown>; user_id: string | null; ip_address: string | null; created_at: string }[]>`
		SELECT id, scope, level, message, stack, context, user_id, ip_address, created_at
		FROM error_logs ORDER BY created_at DESC LIMIT 200
	`;
});

import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import { sql } from '$lib/db';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

async function requireOrgMember(orgId: string) {
	const session = requireSession();
	if (!session.orgs.find((o) => o.id === orgId)) error(403, 'Forbidden');
}

export async function getUsageSummary(orgId: string) {
	await requireOrgMember(orgId);
	const rows = await sql`
		SELECT resource_type,
		       SUM(quantity) as total,
		       unit
		FROM usage_events
		WHERE org_id = ${orgId}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type, unit
		ORDER BY resource_type
	`;
	return rows;
}

export async function getUsageByProject(orgId: string) {
	await requireOrgMember(orgId);
	const rows = await sql`
		SELECT project_id, resource_type,
		       SUM(quantity) as total, unit
		FROM usage_events
		WHERE org_id = ${orgId}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY project_id, resource_type, unit
		ORDER BY project_id, resource_type
	`;
	return rows;
}

export async function getInvoices(orgId: string) {
	await requireOrgMember(orgId);
	return sql`
		SELECT id, period_start, period_end, total_cents, status, created_at
		FROM invoices
		WHERE org_id = ${orgId}
		ORDER BY created_at DESC
		LIMIT 24
	`;
}

export async function getAdminUsageAll() {
	const session = requireSession();
	if (!session.isAdmin) error(403, 'Admin required');
	return sql`
		SELECT o.slug as org_slug, e.resource_type,
		       SUM(e.quantity) as total, e.unit
		FROM usage_events e
		JOIN organizations o ON o.id = e.org_id
		WHERE e.recorded_at >= date_trunc('month', now())
		GROUP BY o.slug, e.resource_type, e.unit
		ORDER BY o.slug, e.resource_type
	`;
}

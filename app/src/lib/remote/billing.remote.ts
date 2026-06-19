import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { sql } from '$lib/db';

function requireOrgMember(orgId: string) {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	if (!locals.session.orgs.find((o) => o.id === orgId)) error(403, 'Forbidden');
}

export const getUsageSummary = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	return sql`
		SELECT resource_type, SUM(quantity) as total, unit
		FROM usage_events
		WHERE org_id = ${params.org!}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type, unit
		ORDER BY resource_type
	`;
});

export const getUsageByProject = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	return sql`
		SELECT project_id, resource_type, SUM(quantity) as total, unit
		FROM usage_events
		WHERE org_id = ${params.org!}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY project_id, resource_type, unit
		ORDER BY project_id, resource_type
	`;
});

export const getInvoices = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	return sql`
		SELECT id, period_start, period_end, total_cents, status, created_at
		FROM invoices
		WHERE org_id = ${params.org!}
		ORDER BY created_at DESC
		LIMIT 24
	`;
});

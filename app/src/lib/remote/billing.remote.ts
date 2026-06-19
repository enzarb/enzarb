import { query } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { sql } from '$lib/db';

function resolveNamespace() {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.slug === params.namespace);
	if (!org) error(403, 'Forbidden');
	return org;
}

export const getUsageSummary = query(async () => {
	const org = resolveNamespace();
	return sql`
		SELECT resource_type, SUM(quantity) as total, unit
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type, unit
		ORDER BY resource_type
	`;
});

export const getUsageByProject = query(async () => {
	const org = resolveNamespace();
	return sql`
		SELECT project_id, resource_type, SUM(quantity) as total, unit
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY project_id, resource_type, unit
		ORDER BY project_id, resource_type
	`;
});

export const getInvoices = query(async () => {
	const org = resolveNamespace();
	return sql`
		SELECT id, period_start, period_end, total_cents, status, created_at
		FROM invoices
		WHERE org_id = ${org.id}
		ORDER BY created_at DESC
		LIMIT 24
	`;
});

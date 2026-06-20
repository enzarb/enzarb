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

// Live estimate of this month's spend, computed from usage_events against the
// admin-editable pricing in app_settings. Mirrors the tiered math in the billing
// worker (billing/cmd/billing) so users see expenses before the monthly invoice
// is cut. Returns dollars; `cpu`/`mem` reflect free-allowance deductions.
export const getEstimatedCost = query(async () => {
	const org = resolveNamespace();

	const usageRows = await sql`
		SELECT resource_type, SUM(quantity)::float8 AS total
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type
	`;
	const usage: Record<string, number> = {};
	for (const r of usageRows) usage[r.resource_type] = Number(r.total);

	const priceRows = await sql`SELECT key, value FROM app_settings WHERE key LIKE 'pricing_%'`;
	const p: Record<string, number> = {};
	for (const r of priceRows) p[r.key] = Number(r.value);

	const cpuBillable = Math.max(0, (usage.cpu_seconds ?? 0) - (p.pricing_free_cpu_seconds ?? 0));
	const memBillable = Math.max(0, (usage.mem_gib_seconds ?? 0) - (p.pricing_free_mem_gib_seconds ?? 0));

	const GIB = 1073741824; // bytes per GiB; network usage is recorded in bytes
	const lines = {
		cpu: cpuBillable * (p.pricing_cpu_seconds_per_unit ?? 0),
		mem: memBillable * (p.pricing_mem_gib_seconds_per_unit ?? 0),
		net_in: ((usage.net_ingress_bytes ?? 0) / GIB) * (p.pricing_net_ingress_per_gib ?? 0),
		net_out: ((usage.net_egress_bytes ?? 0) / GIB) * (p.pricing_net_egress_per_gib ?? 0),
		storage: (usage.storage_gib_seconds ?? 0) * (p.pricing_storage_gib_seconds_per_unit ?? 0)
	};
	const total = Object.values(lines).reduce((a, b) => a + b, 0);

	return { total, lines };
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

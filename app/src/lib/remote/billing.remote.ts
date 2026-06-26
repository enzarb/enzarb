import { query } from '$app/server';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { RESOURCE_TYPES, COMPONENTS } from '$lib/billing';
import { resolveOrg } from './guard';

const GIB = 1073741824; // bytes per GiB

// Pricing keys read from app_settings, mirroring the billing worker
// (billing/cmd/billing) so the live estimate matches the issued invoice.
type Pricing = Record<string, number>;

async function loadPricing(): Promise<Pricing> {
	const rows = await sql`SELECT key, value FROM app_settings WHERE key LIKE 'pricing_%'`;
	const p: Pricing = {};
	for (const r of rows) p[r.key] = Number(r.value);
	return p;
}

// costForResource computes the dollar cost of a single resource type given a raw
// quantity. Free allowances (cpu/mem) are NOT applied here — getEstimatedCost
// applies the org-wide free tier at the aggregate level. This is the single
// source of truth shared by the per-project rollup and the time series.
function costForResource(resourceType: string, quantity: number, p: Pricing): number {
	switch (resourceType) {
		case 'cpu_seconds':
			return quantity * (p.pricing_cpu_seconds_per_unit ?? 0);
		case 'mem_gib_seconds':
			return quantity * (p.pricing_mem_gib_seconds_per_unit ?? 0);
		case 'net_ingress_internal_bytes':
			return (quantity / GIB) * (p.pricing_net_ingress_internal_per_gib ?? 0);
		case 'net_egress_internal_bytes':
			return (quantity / GIB) * (p.pricing_net_egress_internal_per_gib ?? 0);
		case 'net_ingress_external_bytes':
			return (quantity / GIB) * (p.pricing_net_ingress_external_per_gib ?? 0);
		case 'net_egress_external_bytes':
			return (quantity / GIB) * (p.pricing_net_egress_external_per_gib ?? 0);
		case 'storage_gib_seconds':
			return quantity * (p.pricing_storage_gib_seconds_per_unit ?? 0);
		case 'zot_storage_gib_seconds':
			return quantity * (p.pricing_zot_storage_gib_seconds_per_unit ?? 0);
		default:
			return 0;
	}
}

export const getUsageSummary = query(async () => {
	const org = resolveOrg();
	return sql`
		SELECT resource_type, SUM(quantity) as total, unit
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type, unit
		ORDER BY resource_type
	`;
});

// Raw usage quantities for the current month plus free-tier limits, so the
// UI can show "X% of free plan" meters regardless of whether there's a cost.
export const getUsageWithLimits = query(async () => {
	const org = resolveOrg();
	const usageRows = await sql<{ resource_type: string; total: number }[]>`
		SELECT resource_type, SUM(quantity)::float8 AS total
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type
	`;
	const usage: Record<string, number> = {};
	for (const r of usageRows) usage[r.resource_type] = Number(r.total);

	const p = await loadPricing();
	return {
		cpu_seconds: usage.cpu_seconds ?? 0,
		mem_gib_seconds: usage.mem_gib_seconds ?? 0,
		storage_gib_seconds: usage.storage_gib_seconds ?? 0,
		zot_storage_gib_seconds: usage.zot_storage_gib_seconds ?? 0,
		net_ingress_internal_bytes: usage.net_ingress_internal_bytes ?? 0,
		net_egress_internal_bytes: usage.net_egress_internal_bytes ?? 0,
		net_ingress_external_bytes: usage.net_ingress_external_bytes ?? 0,
		net_egress_external_bytes: usage.net_egress_external_bytes ?? 0,
		free_cpu_seconds: p.pricing_free_cpu_seconds ?? 0,
		free_mem_gib_seconds: p.pricing_free_mem_gib_seconds ?? 0,
	};
});

// Live estimate of this month's spend, computed from usage_events against the
// admin-editable pricing in app_settings. Mirrors the tiered math in the billing
// worker so users see expenses before the monthly invoice is cut. Returns dollars;
// `cpu`/`mem` reflect free-allowance deductions.
export const getEstimatedCost = query(async () => {
	const org = resolveOrg();

	const usageRows = await sql`
		SELECT resource_type, SUM(quantity)::float8 AS total
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY resource_type
	`;
	const usage: Record<string, number> = {};
	for (const r of usageRows) usage[r.resource_type] = Number(r.total);

	const p = await loadPricing();

	const cpuBillable = Math.max(0, (usage.cpu_seconds ?? 0) - (p.pricing_free_cpu_seconds ?? 0));
	const memBillable = Math.max(0, (usage.mem_gib_seconds ?? 0) - (p.pricing_free_mem_gib_seconds ?? 0));

	const lines = {
		cpu: cpuBillable * (p.pricing_cpu_seconds_per_unit ?? 0),
		mem: memBillable * (p.pricing_mem_gib_seconds_per_unit ?? 0),
		net_in_internal: costForResource('net_ingress_internal_bytes', usage.net_ingress_internal_bytes ?? 0, p),
		net_out_internal: costForResource('net_egress_internal_bytes', usage.net_egress_internal_bytes ?? 0, p),
		net_in_external: costForResource('net_ingress_external_bytes', usage.net_ingress_external_bytes ?? 0, p),
		net_out_external: costForResource('net_egress_external_bytes', usage.net_egress_external_bytes ?? 0, p),
		storage: costForResource('storage_gib_seconds', usage.storage_gib_seconds ?? 0, p),
		zot: costForResource('zot_storage_gib_seconds', usage.zot_storage_gib_seconds ?? 0, p)
	};
	const total = Object.values(lines).reduce((a, b) => a + b, 0);

	return { total, lines };
});

// One row per project, with each resource type pivoted into a column plus the
// estimated cost for the project this month. Free allowances are org-wide so they
// are not applied per project here (the top-line estimate handles them).
export const getProjectRollup = query(async () => {
	const org = resolveOrg();
	const rows = await sql<
		{
			project_id: string;
			cpu_seconds: number;
			mem_gib_seconds: number;
			net_ingress_internal_bytes: number;
			net_egress_internal_bytes: number;
			net_ingress_external_bytes: number;
			net_egress_external_bytes: number;
			storage_gib_seconds: number;
			zot_storage_gib_seconds: number;
		}[]
	>`
		SELECT
			project_id,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'cpu_seconds'), 0)::float8 AS cpu_seconds,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'mem_gib_seconds'), 0)::float8 AS mem_gib_seconds,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_ingress_internal_bytes'), 0)::float8 AS net_ingress_internal_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_egress_internal_bytes'), 0)::float8 AS net_egress_internal_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_ingress_external_bytes'), 0)::float8 AS net_ingress_external_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_egress_external_bytes'), 0)::float8 AS net_egress_external_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'storage_gib_seconds'), 0)::float8 AS storage_gib_seconds,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'zot_storage_gib_seconds'), 0)::float8 AS zot_storage_gib_seconds
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY project_id
		ORDER BY project_id
	`;

	const p = await loadPricing();
	return rows.map((r) => {
		const cost = RESOURCE_TYPES.reduce(
			(sum, rt) => sum + costForResource(rt, Number(r[rt] ?? 0), p),
			0
		);
		return { ...r, cost };
	});
});

// Monthly cost grouped by component, so the dashboard can split workspace vs
// deploy-environment spend (and surface Zot registry usage).
export const getCostByComponent = query(async () => {
	const org = resolveOrg();
	const rows = await sql`
		SELECT component, resource_type, SUM(quantity)::float8 AS total
		FROM usage_events
		WHERE org_id = ${org.id}
		  AND recorded_at >= date_trunc('month', now())
		GROUP BY component, resource_type
	`;
	const p = await loadPricing();
	const byComponent: Record<string, number> = {};
	for (const r of rows) {
		byComponent[r.component] =
			(byComponent[r.component] ?? 0) + costForResource(r.resource_type, Number(r.total), p);
	}
	return COMPONENTS.map((component) => ({ component, cost: byComponent[component] ?? 0 }));
});

// Daily cost buckets for the stacked bar chart, optionally filtered by project
// and/or resource type. Returns one entry per day with per-resource-type cost
// segments so the client can stack by resource type. Filters are whitelisted
// server-side (resource types via the enum, project IDs scoped to the org).
export const getCostTimeSeries = query(
	z.object({
		days: z.number().int().min(1).max(90).default(30),
		projectIds: z.array(z.string()).default([]),
		resourceTypes: z.array(z.enum(RESOURCE_TYPES)).default([])
	}),
	async ({ days, projectIds, resourceTypes }) => {
		const org = resolveOrg();
		const since = new Date(Date.now() - days * 86400000);

		const rows = await sql<{ day: Date; resource_type: string; total: number }[]>`
			SELECT date_trunc('day', recorded_at) AS day, resource_type, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND recorded_at >= ${since}
			  ${projectIds.length ? sql`AND project_id = ANY(${projectIds})` : sql``}
			  ${resourceTypes.length ? sql`AND resource_type = ANY(${resourceTypes as unknown as string[]})` : sql``}
			GROUP BY day, resource_type
			ORDER BY day
		`;

		const p = await loadPricing();
		// Collapse into one bucket per day with per-resource-type cost segments.
		const buckets = new Map<string, Record<string, number>>();
		for (const r of rows) {
			const key = new Date(r.day).toISOString().slice(0, 10);
			const seg = buckets.get(key) ?? {};
			seg[r.resource_type] =
				(seg[r.resource_type] ?? 0) + costForResource(r.resource_type, Number(r.total), p);
			buckets.set(key, seg);
		}
		// Emit exactly `days` buckets ending today, zero-filling days with no usage
		// so the chart always shows the full selected range as distinct columns.
		// Segments are always emitted in RESOURCE_TYPES order so the chart stacks
		// consistently across all buckets.
		const activeTypes = resourceTypes.length ? resourceTypes : [...RESOURCE_TYPES];
		const start = new Date(Date.now() - (days - 1) * 86400000);
		return Array.from({ length: days }, (_, i) => {
			const day = new Date(start.getTime() + i * 86400000).toISOString().slice(0, 10);
			const raw = buckets.get(day) ?? {};
			const segments: Record<string, number> = {};
			for (const rt of activeTypes) {
				if (raw[rt]) segments[rt] = raw[rt];
			}
			return { day, segments };
		});
	}
);

export const getInvoices = query(async () => {
	const org = resolveOrg();
	return sql`
		SELECT id, period_start, period_end, total_cents, status, created_at
		FROM invoices
		WHERE org_id = ${org.id}
		ORDER BY created_at DESC
		LIMIT 24
	`;
});

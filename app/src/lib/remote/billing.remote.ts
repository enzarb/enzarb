import { query } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { RESOURCE_TYPES, COMPONENTS } from '$lib/billing';
import { buildInvoicePdf } from '$lib/invoicePdf';
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
		case 'vcpu_hours':
			return quantity * (p.pricing_vcpu_hours_per_unit ?? 0);
		case 'mem_gib_hours':
			return quantity * (p.pricing_mem_gib_hours_per_unit ?? 0);
		case 'net_ingress_internal_bytes':
			return (quantity / GIB) * (p.pricing_net_ingress_internal_per_gib ?? 0);
		case 'net_egress_internal_bytes':
			return (quantity / GIB) * (p.pricing_net_egress_internal_per_gib ?? 0);
		case 'net_ingress_external_bytes':
			return (quantity / GIB) * (p.pricing_net_ingress_external_per_gib ?? 0);
		case 'net_egress_external_bytes':
			return (quantity / GIB) * (p.pricing_net_egress_external_per_gib ?? 0);
		case 'block_storage_gib_months':
			return quantity * (p.pricing_block_storage_gib_months_per_unit ?? 0);
		case 'registry_gib_months':
			return quantity * (p.pricing_registry_gib_months_per_unit ?? 0);
		default:
			return 0;
	}
}

// Public (no-auth) query that exposes the current per-unit rates and free-tier
// allowances — used on the home page pricing block and the billing page rate labels.
export const getPublicPricing = query(async () => {
	const p = await loadPricing();
	return {
		vcpuHoursPerUnit: p.pricing_vcpu_hours_per_unit ?? 0,
		memGiBHoursPerUnit: p.pricing_mem_gib_hours_per_unit ?? 0,
		blockStorageGiBMonthsPerUnit: p.pricing_block_storage_gib_months_per_unit ?? 0,
		registryGiBMonthsPerUnit: p.pricing_registry_gib_months_per_unit ?? 0,
		netEgressExternalPerGib: p.pricing_net_egress_external_per_gib ?? 0,
		freeVCPUHours: p.pricing_free_vcpu_hours ?? 0,
		freeMemGiBHours: p.pricing_free_mem_gib_hours ?? 0,
		freeBlockStorageGiBMonths: p.pricing_free_block_storage_gib_months ?? 0,
		freeRegistryGiBMonths: p.pricing_free_registry_gib_months ?? 0,
		freeNetEgressExternalGib: p.pricing_free_net_egress_external_gib ?? 0,
	};
});

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
		vcpu_hours: usage.vcpu_hours ?? 0,
		mem_gib_hours: usage.mem_gib_hours ?? 0,
		block_storage_gib_months: usage.block_storage_gib_months ?? 0,
		registry_gib_months: usage.registry_gib_months ?? 0,
		net_ingress_internal_bytes: usage.net_ingress_internal_bytes ?? 0,
		net_egress_internal_bytes: usage.net_egress_internal_bytes ?? 0,
		net_ingress_external_bytes: usage.net_ingress_external_bytes ?? 0,
		net_egress_external_bytes: usage.net_egress_external_bytes ?? 0,
		free_vcpu_hours: p.pricing_free_vcpu_hours ?? 0,
		free_mem_gib_hours: p.pricing_free_mem_gib_hours ?? 0,
		free_block_storage_gib_months: p.pricing_free_block_storage_gib_months ?? 0,
		free_registry_gib_months: p.pricing_free_registry_gib_months ?? 0,
		// Network free allowances are configured in GiB; expose as bytes so the
		// UI meters compare against the raw byte usage above.
		free_net_ingress_internal_bytes: (p.pricing_free_net_ingress_internal_gib ?? 0) * GIB,
		free_net_egress_internal_bytes: (p.pricing_free_net_egress_internal_gib ?? 0) * GIB,
		free_net_ingress_external_bytes: (p.pricing_free_net_ingress_external_gib ?? 0) * GIB,
		free_net_egress_external_bytes: (p.pricing_free_net_egress_external_gib ?? 0) * GIB
	};
});

// Live estimate of this month's spend, computed from usage_events against the
// admin-editable pricing in app_settings. Mirrors the tiered math in the billing
// worker so users see expenses before the monthly invoice is cut. Returns dollars;
// every line reflects its per-metric free-allowance deduction.
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

	// Free allowances are org-wide and applied here at the aggregate level, one
	// per billed metric. Compute/storage allowances are in the metric's native
	// unit; network allowances are in GiB (converted to bytes to match usage).
	const billable = (used: number, free: number) => Math.max(0, used - free);
	const netBillableGib = (usedBytes: number, freeGib: number) =>
		Math.max(0, usedBytes / GIB - freeGib);

	const lines = {
		cpu:
			billable(usage.vcpu_hours ?? 0, p.pricing_free_vcpu_hours ?? 0) *
			(p.pricing_vcpu_hours_per_unit ?? 0),
		mem:
			billable(usage.mem_gib_hours ?? 0, p.pricing_free_mem_gib_hours ?? 0) *
			(p.pricing_mem_gib_hours_per_unit ?? 0),
		net_in_internal:
			netBillableGib(usage.net_ingress_internal_bytes ?? 0, p.pricing_free_net_ingress_internal_gib ?? 0) *
			(p.pricing_net_ingress_internal_per_gib ?? 0),
		net_out_internal:
			netBillableGib(usage.net_egress_internal_bytes ?? 0, p.pricing_free_net_egress_internal_gib ?? 0) *
			(p.pricing_net_egress_internal_per_gib ?? 0),
		net_in_external:
			netBillableGib(usage.net_ingress_external_bytes ?? 0, p.pricing_free_net_ingress_external_gib ?? 0) *
			(p.pricing_net_ingress_external_per_gib ?? 0),
		net_out_external:
			netBillableGib(usage.net_egress_external_bytes ?? 0, p.pricing_free_net_egress_external_gib ?? 0) *
			(p.pricing_net_egress_external_per_gib ?? 0),
		storage:
			billable(usage.block_storage_gib_months ?? 0, p.pricing_free_block_storage_gib_months ?? 0) *
			(p.pricing_block_storage_gib_months_per_unit ?? 0),
		zot:
			billable(usage.registry_gib_months ?? 0, p.pricing_free_registry_gib_months ?? 0) *
			(p.pricing_registry_gib_months_per_unit ?? 0)
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
			vcpu_hours: number;
			mem_gib_hours: number;
			net_ingress_internal_bytes: number;
			net_egress_internal_bytes: number;
			net_ingress_external_bytes: number;
			net_egress_external_bytes: number;
			block_storage_gib_months: number;
			registry_gib_months: number;
		}[]
	>`
		SELECT
			project_id,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'vcpu_hours'), 0)::float8 AS vcpu_hours,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'mem_gib_hours'), 0)::float8 AS mem_gib_hours,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_ingress_internal_bytes'), 0)::float8 AS net_ingress_internal_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_egress_internal_bytes'), 0)::float8 AS net_egress_internal_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_ingress_external_bytes'), 0)::float8 AS net_ingress_external_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'net_egress_external_bytes'), 0)::float8 AS net_egress_external_bytes,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'block_storage_gib_months'), 0)::float8 AS block_storage_gib_months,
			COALESCE(SUM(quantity) FILTER (WHERE resource_type = 'registry_gib_months'), 0)::float8 AS registry_gib_months
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

		// Bucket days in UTC explicitly — date_trunc uses the DB session
		// timezone, and the zero-filled day keys below are UTC; a mismatch
		// splits one day's usage across two chart bars (sawtooth).
		const rows = await sql<{ day: string; resource_type: string; total: number }[]>`
			SELECT to_char(recorded_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS day, resource_type, SUM(quantity)::float8 AS total
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
			const key = r.day;
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

// Per-resource drill-down for a single project: workloads (pods), PVCs, and
// registry images broken out by label, with per-row cost estimates.
export const getProjectDetail = query(
	z.object({ projectId: z.string() }),
	async ({ projectId }) => {
		const org = resolveOrg();
		const p = await loadPricing();

		// Per-pod: CPU, memory, and all four network directions, plus the K8s
		// owner so the UI can group by Deployment/StatefulSet/etc.
		const workloadRows = await sql<{
			label: string;
			owner: string | null;
			resource_type: string;
			total: number;
		}[]>`
			SELECT label, MAX(owner) AS owner, resource_type, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND project_id = ${projectId}
			  AND resource_type IN (
			    'vcpu_hours', 'mem_gib_hours',
			    'net_ingress_internal_bytes', 'net_egress_internal_bytes',
			    'net_ingress_external_bytes', 'net_egress_external_bytes'
			  )
			  AND recorded_at >= date_trunc('month', now())
			  AND label IS NOT NULL AND label != ''
			GROUP BY label, resource_type
			ORDER BY label
		`;

		const storageRows = await sql<{ label: string; total: number }[]>`
			SELECT label, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND project_id = ${projectId}
			  AND resource_type = 'block_storage_gib_months'
			  AND recorded_at >= date_trunc('month', now())
			  AND label IS NOT NULL AND label != ''
			GROUP BY label
			ORDER BY label
		`;

		const imageRows = await sql<{ label: string; total: number }[]>`
			SELECT label, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND project_id = ${projectId}
			  AND resource_type = 'registry_gib_months'
			  AND recorded_at >= date_trunc('month', now())
			  AND label IS NOT NULL AND label != ''
			GROUP BY label
			ORDER BY label
		`;

		type WorkloadEntry = {
			label: string;
			owner: string;
			vcpu_hours: number;
			mem_gib_hours: number;
			net_ingress_internal_bytes: number;
			net_egress_internal_bytes: number;
			net_ingress_external_bytes: number;
			net_egress_external_bytes: number;
			cost: number;
		};
		const workloads = new Map<string, WorkloadEntry>();
		for (const r of workloadRows) {
			const entry = workloads.get(r.label) ?? {
				label: r.label,
				owner: r.owner ?? '',
				vcpu_hours: 0,
				mem_gib_hours: 0,
				net_ingress_internal_bytes: 0,
				net_egress_internal_bytes: 0,
				net_ingress_external_bytes: 0,
				net_egress_external_bytes: 0,
				cost: 0
			};
			(entry as unknown as Record<string, number>)[r.resource_type] = Number(r.total);
			entry.cost += costForResource(r.resource_type, Number(r.total), p);
			workloads.set(r.label, entry);
		}

		return {
			workloads: [...workloads.values()],
			storage: storageRows.map((r) => ({
				label: r.label,
				gib_months: Number(r.total),
				cost: costForResource('block_storage_gib_months', Number(r.total), p)
			})),
			images: imageRows.map((r) => ({
				label: r.label,
				gib_months: Number(r.total),
				cost: costForResource('registry_gib_months', Number(r.total), p)
			}))
		};
	}
);

// Daily cost buckets for a single project — same shape as getCostTimeSeries
// but scoped to one project_id so the project billing page can show a chart.
export const getProjectCostTimeSeries = query(
	z.object({
		projectId: z.string(),
		days: z.number().int().min(1).max(90).default(30)
	}),
	async ({ projectId, days }) => {
		const org = resolveOrg();
		const since = new Date(Date.now() - days * 86400000);

		// UTC day bucketing — see getCostTimeSeries.
		const rows = await sql<{ day: string; resource_type: string; total: number }[]>`
			SELECT to_char(recorded_at AT TIME ZONE 'UTC', 'YYYY-MM-DD') AS day, resource_type, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND project_id = ${projectId}
			  AND recorded_at >= ${since}
			GROUP BY day, resource_type
			ORDER BY day
		`;

		const p = await loadPricing();
		const buckets = new Map<string, Record<string, number>>();
		for (const r of rows) {
			const key = r.day;
			const seg = buckets.get(key) ?? {};
			seg[r.resource_type] =
				(seg[r.resource_type] ?? 0) + costForResource(r.resource_type, Number(r.total), p);
			buckets.set(key, seg);
		}
		const start = new Date(Date.now() - (days - 1) * 86400000);
		return Array.from({ length: days }, (_, i) => {
			const day = new Date(start.getTime() + i * 86400000).toISOString().slice(0, 10);
			const raw = buckets.get(day) ?? {};
			const segments: Record<string, number> = {};
			for (const rt of RESOURCE_TYPES) {
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

// Converts a raw usage_events total into a display-friendly line item — same
// unit conventions as the billing worker's stored invoice_line_items (native
// units for compute/storage, GiB for network) so the fallback recompute reads
// the same as a real invoice instead of showing raw byte counts.
function toDisplayLineItem(
	resourceType: string,
	total: number,
	p: Pricing
): { resource_type: string; quantity: number; unit: string; unit_price_cents: number; amount_cents: number } {
	let quantity = total;
	let unit = resourceType;
	let perUnit = 0;
	switch (resourceType) {
		case 'vcpu_hours':
			unit = 'vcpu_hours';
			perUnit = p.pricing_vcpu_hours_per_unit ?? 0;
			break;
		case 'mem_gib_hours':
			unit = 'gib_hours';
			perUnit = p.pricing_mem_gib_hours_per_unit ?? 0;
			break;
		case 'block_storage_gib_months':
			unit = 'gib_months';
			perUnit = p.pricing_block_storage_gib_months_per_unit ?? 0;
			break;
		case 'registry_gib_months':
			unit = 'gib_months';
			perUnit = p.pricing_registry_gib_months_per_unit ?? 0;
			break;
		case 'net_ingress_internal_bytes':
			quantity = total / GIB;
			unit = 'gib';
			perUnit = p.pricing_net_ingress_internal_per_gib ?? 0;
			break;
		case 'net_egress_internal_bytes':
			quantity = total / GIB;
			unit = 'gib';
			perUnit = p.pricing_net_egress_internal_per_gib ?? 0;
			break;
		case 'net_ingress_external_bytes':
			quantity = total / GIB;
			unit = 'gib';
			perUnit = p.pricing_net_ingress_external_per_gib ?? 0;
			break;
		case 'net_egress_external_bytes':
			quantity = total / GIB;
			unit = 'gib';
			perUnit = p.pricing_net_egress_external_per_gib ?? 0;
			break;
	}
	return {
		resource_type: resourceType,
		quantity,
		unit,
		unit_price_cents: perUnit * 100,
		amount_cents: Math.round(quantity * perUnit * 100)
	};
}

// Renders a multipage invoice PDF: a summary page (org, period, total) followed
// by a per-resource-type line item breakdown. Line items come from
// invoice_line_items — the amounts frozen by the billing worker at generation
// time — falling back to a live recompute (at today's rates) for invoices
// issued before that table existed.
export const getInvoicePdf = query(z.object({ invoiceId: z.string().uuid() }), async ({ invoiceId }) => {
	const org = resolveOrg();
	const [invoice] = await sql<
		{ id: string; period_start: Date; period_end: Date; total_cents: number; status: string }[]
	>`
		SELECT id, period_start, period_end, total_cents, status
		FROM invoices
		WHERE id = ${invoiceId} AND org_id = ${org.id}
	`;
	if (!invoice) error(404, 'Invoice not found');

	type LineItem = { resource_type: string; quantity: number; unit: string; unit_price_cents: number; amount_cents: number };
	let lineItems: LineItem[] = await sql<LineItem[]>`
		SELECT resource_type, quantity::float8 AS quantity, unit, unit_price_cents::float8 AS unit_price_cents, amount_cents
		FROM invoice_line_items
		WHERE invoice_id = ${invoiceId}
		ORDER BY resource_type
	`;

	let estimatedLineItems = false;
	if (lineItems.length === 0) {
		estimatedLineItems = true;
		const usageRows = await sql<{ resource_type: string; total: number }[]>`
			SELECT resource_type, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND recorded_at >= ${invoice.period_start}
			  AND recorded_at < ${invoice.period_end}
			GROUP BY resource_type
		`;
		const p = await loadPricing();
		lineItems = usageRows.map((r) => toDisplayLineItem(r.resource_type, r.total, p));
	}

	const [org_row] = await sql<{ display_name: string }[]>`
		SELECT display_name FROM organizations WHERE id = ${org.id}
	`;

	const bytes = await buildInvoicePdf({
		orgName: org_row?.display_name ?? org.id,
		invoiceId: invoice.id,
		periodStart: new Date(invoice.period_start),
		periodEnd: new Date(invoice.period_end),
		status: invoice.status,
		totalCents: Number(invoice.total_cents),
		lineItems,
		estimatedLineItems
	});
	return bytes;
});

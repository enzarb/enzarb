import { query } from '$app/server';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { resolveOrg } from './guard';

// Raw per-minute usage rows for a project, straight from usage_events with no
// cost conversion — powers the Utilization tab's Task-Manager-style charts,
// which show the metering metric itself rather than a dollar rollup.
export const getProjectUtilization = query(
	z.object({
		projectId: z.string(),
		minutes: z.number().int().min(5).max(360).default(60)
	}),
	async ({ projectId, minutes }) => {
		const org = resolveOrg();
		const since = new Date(Date.now() - minutes * 60000);

		// component/environment let the UI label each pod's environment as
		// "workspace" or the deploy environment's slug (test/prod/etc.)
		// instead of the raw, hash-bearing k8s namespace name.
		const rows = await sql<{
			minute: Date;
			resource_type: string;
			label: string | null;
			component: string;
			environment: string | null;
			total: number;
		}[]>`
			SELECT date_trunc('minute', recorded_at) AS minute, resource_type, label, component, environment, SUM(quantity)::float8 AS total
			FROM usage_events
			WHERE org_id = ${org.id}
			  AND project_id = ${projectId}
			  AND recorded_at >= ${since}
			GROUP BY minute, resource_type, label, component, environment
			ORDER BY minute
		`;
		return rows.map((r) => ({
			...r,
			environment: r.component === 'workspace' ? 'workspace' : (r.environment ?? '')
		}));
	}
);

import { query, form } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listEnvironments, createEnvironment, addCustomDomain } from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers } from '$lib/config';

function requireOrgMember(orgId: string, minRole?: 'admin') {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.id === orgId);
	if (!org) error(403, 'Not a member of this organization');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

export const getEnvironments = query(async () => {
	const { params } = getRequestEvent();
	requireOrgMember(params.org!);
	return listEnvironments(params.org!, params.project!);
});

export const createEnv = form(
	z.object({ slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/) }),
	async ({ slug }) => {
		const { params } = getRequestEvent();
		requireOrgMember(params.org!, 'admin');
		const rows = await sql`SELECT tier FROM organizations WHERE id = ${params.org!}`;
		const tier = (rows[0]?.tier ?? 'free') as keyof typeof tiers;
		const existing = await listEnvironments(params.org!, params.project!);
		if (existing.length >= tiers[tier].maxEnvironments) {
			error(422, `Tier limited to ${tiers[tier].maxEnvironments} environment(s)`);
		}
		return createEnvironment(params.org!, params.project!, slug);
	}
);

export const addDomain = form(
	z.object({
		envName: z.string(),
		fqdn: z.string().max(253).regex(/^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}$/)
	}),
	async ({ envName, fqdn }) => {
		const { params } = getRequestEvent();
		requireOrgMember(params.org!, 'admin');
		return addCustomDomain(params.org!, envName, fqdn);
	}
);

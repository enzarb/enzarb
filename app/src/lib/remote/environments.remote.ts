import { query, form } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listEnvironments, createEnvironment, addCustomDomain } from '$lib/k8s';
import { sql } from '$lib/db';
import { tiers } from '$lib/config';

function resolveNamespace(minRole?: 'admin') {
	const { locals, params } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	const org = locals.session.orgs.find((o) => o.slug === params.namespace);
	if (!org) error(403, 'Not a member of this organization');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

export const getEnvironments = query(async () => {
	const { params } = getRequestEvent();
	const org = resolveNamespace();
	return listEnvironments(org.id, params.project!);
});

export const createEnv = form(
	z.object({ slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/) }),
	async ({ slug }) => {
		const { params } = getRequestEvent();
		const org = resolveNamespace('admin');
		const rows = await sql`SELECT tier FROM organizations WHERE id = ${org.id}`;
		const tier = (rows[0]?.tier ?? 'free') as keyof typeof tiers;
		const existing = await listEnvironments(org.id, params.project!);
		if (existing.length >= tiers[tier].maxEnvironments) {
			error(422, `Tier limited to ${tiers[tier].maxEnvironments} environment(s)`);
		}
		return createEnvironment(org.id, params.project!, slug);
	}
);

export const addDomain = form(
	z.object({
		envName: z.string(),
		fqdn: z.string().max(253).regex(/^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}$/)
	}),
	async ({ envName, fqdn }) => {
		const org = resolveNamespace('admin');
		return addCustomDomain(org.id, envName, fqdn);
	}
);

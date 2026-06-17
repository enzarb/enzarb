import { error } from '@sveltejs/kit';
import { getRequestEvent } from '$app/server';
import { listEnvironments, createEnvironment, addCustomDomain } from '$lib/k8s';
import { tiers } from '$lib/config';
import { sql } from '$lib/db';
import { z } from 'zod/v4';

function requireSession() {
	const { locals } = getRequestEvent();
	if (!locals.session) error(401, 'Unauthorized');
	return locals.session;
}

async function requireOrgMember(orgId: string, minRole?: 'admin') {
	const session = requireSession();
	const org = session.orgs.find((o) => o.id === orgId);
	if (!org) error(403, 'Not a member of this organization');
	if (minRole === 'admin' && org.role === 'member') error(403, 'Admin required');
	return org;
}

export async function getEnvironments(orgId: string, projectSlug: string) {
	await requireOrgMember(orgId);
	return listEnvironments(orgId, projectSlug);
}

const CreateEnvSchema = z.object({
	orgId: z.string(),
	projectSlug: z.string(),
	slug: z.string().regex(/^[a-z0-9-]+$/)
});

export async function createEnv(input: z.infer<typeof CreateEnvSchema>) {
	const parsed = CreateEnvSchema.parse(input);
	await requireOrgMember(parsed.orgId, 'admin');

	const rows = await sql`SELECT tier FROM organizations WHERE id = ${parsed.orgId}`;
	const tier = (rows[0]?.tier ?? 'free') as keyof typeof tiers;
	const existing = await listEnvironments(parsed.orgId, parsed.projectSlug);
	if (existing.length >= tiers[tier].maxEnvironments) {
		error(422, `Tier limited to ${tiers[tier].maxEnvironments} environment(s)`);
	}

	return createEnvironment(parsed.orgId, parsed.projectSlug, parsed.slug);
}

const AddDomainSchema = z.object({
	orgId: z.string(),
	envName: z.string(),
	fqdn: z.string()
});

export async function addDomain(input: z.infer<typeof AddDomainSchema>) {
	const parsed = AddDomainSchema.parse(input);
	await requireOrgMember(parsed.orgId, 'admin');
	return addCustomDomain(parsed.orgId, parsed.envName, parsed.fqdn);
}

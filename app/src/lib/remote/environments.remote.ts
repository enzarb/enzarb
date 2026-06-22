import { query, form, command } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { listEnvironments, createEnvironment, addCustomDomain, setDefaultEnvironment } from '$lib/k8s';
import { getProject } from './projects.remote';
import { sql } from '$lib/db';
import { tiers, config } from '$lib/config';
import { resolveOrg, requirePrivilege } from './guard';

export const getEnvironments = query(async () => {
	const { params } = getRequestEvent();
	const org = resolveOrg();
	const envs = await listEnvironments(org.id, params.project!);
	const deployZone = `env.${config.domain}`;
	return { envs, deployZone };
});

export const createEnv = form(
	z.object({ slug: z.string().min(1).max(63).regex(/^[a-z0-9-]+$/) }),
	async ({ slug }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		const rows = await sql`SELECT tier FROM organizations WHERE id = ${org.id}`;
		const tier = (rows[0]?.tier ?? 'free') as keyof typeof tiers;
		const existing = await listEnvironments(org.id, params.project!);
		if (existing.length >= tiers[tier].maxEnvironments) {
			error(422, `Tier limited to ${tiers[tier].maxEnvironments} environment(s)`);
		}
		const result = await createEnvironment(org.id, params.project!, slug);
		await getEnvironments().refresh();
		return result;
	}
);

export const setDefaultEnv = command(
	z.object({ envSlug: z.string().nullable() }),
	async ({ envSlug }) => {
		const { params } = getRequestEvent();
		const org = requirePrivilege('environment.manage');
		await setDefaultEnvironment(org.id, params.project!, envSlug);
		await Promise.all([getEnvironments().refresh(), getProject().refresh()]);
	}
);

export const addDomain = form(
	z.object({
		envName: z.string(),
		fqdn: z.string().max(253).regex(/^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}$/)
	}),
	async ({ envName, fqdn }) => {
		const org = requirePrivilege('environment.manage');
		const result = await addCustomDomain(org.id, envName, fqdn);
		await getEnvironments().refresh();
		return result;
	}
);

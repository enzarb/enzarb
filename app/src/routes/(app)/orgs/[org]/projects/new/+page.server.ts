import { redirect, fail, error } from '@sveltejs/kit';
import type { PageServerLoad, Actions } from './$types';
import { createNewProject } from '$remote/projects';
import { tiers } from '$lib/config';
import { sql } from '$lib/db';

export const load: PageServerLoad = async ({ params, locals }) => {
	const session = locals.session!;
	const org = session.orgs.find((o) => o.id === params.org);
	if (!org) error(403, 'Forbidden');
	const rows = await sql`SELECT tier FROM organizations WHERE id = ${params.org}`;
	const tier = (rows[0]?.tier ?? 'free') as keyof typeof tiers;
	return { org: { ...org, tier }, limits: tiers[tier] };
};

export const actions: Actions = {
	default: async ({ request, params, locals }) => {
		const session = locals.session!;
		const org = session.orgs.find((o) => o.id === params.org);
		if (!org) error(403, 'Forbidden');

		const data = await request.formData();
		const slug = data.get('slug') as string;
		const displayName = data.get('displayName') as string;
		const toolNames = data.getAll('tools') as string[];
		const storageGi = parseInt(data.get('storageGi') as string, 10) || 10;

		const tools = toolNames.map((name) => ({ name, version: 'latest' }));

		try {
			await createNewProject({ orgId: params.org, slug, displayName, tools, storageGi });
		} catch (e: any) {
			return fail(422, { error: e?.body?.message ?? e.message ?? 'Failed to create project' });
		}

		redirect(302, `/orgs/${params.org}/projects/${slug}`);
	}
};

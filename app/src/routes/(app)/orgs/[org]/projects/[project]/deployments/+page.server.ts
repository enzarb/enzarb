import type { PageServerLoad, Actions } from './$types';
import { fail, error } from '@sveltejs/kit';
import { getEnvironments, createEnv, addDomain } from '$remote/environments';

export const load: PageServerLoad = async ({ params }) => {
	const envs = await getEnvironments(params.org, params.project);
	return { envs };
};

export const actions: Actions = {
	createEnv: async ({ request, params }) => {
		const data = await request.formData();
		const slug = data.get('slug') as string;
		try {
			await createEnv({ orgId: params.org, projectSlug: params.project, slug });
		} catch (e: any) {
			return fail(422, { error: e?.body?.message ?? e.message });
		}
		return { success: true };
	},
	addDomain: async ({ request, params }) => {
		const data = await request.formData();
		const envName = data.get('envName') as string;
		const fqdn = data.get('fqdn') as string;
		try {
			await addDomain({ orgId: params.org, envName, fqdn });
		} catch (e: any) {
			return fail(422, { error: e?.body?.message ?? e.message });
		}
		return { success: true };
	}
};

import type { PageServerLoad, Actions } from './$types';
import { error, fail } from '@sveltejs/kit';
import { listOrgs, listUsers, createOrg, setOrgTier, inviteMember } from '$remote/admin';

export const load: PageServerLoad = async ({ locals }) => {
	if (!locals.session?.isAdmin) error(403, 'Admin required');
	const [orgs, users] = await Promise.all([listOrgs(), listUsers()]);
	return { orgs, users };
};

export const actions: Actions = {
	createOrg: async ({ request, locals }) => {
		if (!locals.session?.isAdmin) error(403, 'Admin required');
		const data = await request.formData();
		try {
			await createOrg({
				slug: data.get('slug') as string,
				displayName: data.get('displayName') as string,
				tier: (data.get('tier') as any) ?? 'free'
			});
		} catch (e: any) {
			return fail(422, { error: e.message });
		}
		return { success: true };
	},
	invite: async ({ request, locals }) => {
		if (!locals.session?.isAdmin) error(403, 'Admin required');
		const data = await request.formData();
		try {
			await inviteMember({
				orgId: data.get('orgId') as string,
				email: data.get('email') as string,
				role: (data.get('role') as any) ?? 'member'
			});
		} catch (e: any) {
			return fail(422, { error: e.message });
		}
		return { success: true };
	},
	setTier: async ({ request, locals }) => {
		if (!locals.session?.isAdmin) error(403, 'Admin required');
		const data = await request.formData();
		try {
			await setOrgTier({ orgId: data.get('orgId') as string, tier: data.get('tier') as any });
		} catch (e: any) {
			return fail(422, { error: e.message });
		}
		return { success: true };
	}
};

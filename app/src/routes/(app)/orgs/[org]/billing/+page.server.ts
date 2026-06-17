import type { PageServerLoad } from './$types';
import { error } from '@sveltejs/kit';
import { getUsageSummary, getUsageByProject, getInvoices } from '$remote/billing';

export const load: PageServerLoad = async ({ params, locals }) => {
	const session = locals.session!;
	if (!session.orgs.find((o) => o.id === params.org)) error(403, 'Forbidden');
	const [summary, byProject, invoices] = await Promise.all([
		getUsageSummary(params.org),
		getUsageByProject(params.org),
		getInvoices(params.org)
	]);
	return { summary, byProject, invoices };
};

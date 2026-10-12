import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

// Tabs used to live in `?tab=`; they are routes now. Old links keep working (308), and an unknown
// `tab` just drops the parameter (it always meant Overview).
const LEGACY_TABS = new Set(['terminal', 'settings']);

export const load: PageServerLoad = ({ url, params }) => {
	const tab = url.searchParams.get('tab');
	if (tab === null) return;
	const rest = new URLSearchParams(url.searchParams);
	rest.delete('tab');
	const base = `/projects/${encodeURIComponent(params.name)}`;
	const path = tab && LEGACY_TABS.has(tab) ? `${base}/${tab}` : base;
	redirect(308, rest.size ? `${path}?${rest}` : path);
};

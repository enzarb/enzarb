import { error } from '@sveltejs/kit';
import { dev } from '$app/env';
import type { LayoutServerLoad } from './$types';

/** Developer-only pages (the component gallery): not reachable in a production build. */
export const load: LayoutServerLoad = () => {
	if (!dev) error(404, 'Not found');
};

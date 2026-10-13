import { redirect } from '@sveltejs/kit';
import type { PageServerLoad } from './$types';

/** Claude setup moved to the user's settings (one token per user); keeps old links working. */
export const load: PageServerLoad = () => redirect(308, '/settings#claude');

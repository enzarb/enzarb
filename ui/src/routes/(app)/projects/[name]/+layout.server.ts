import { chatEnabled } from '#lib/server/flags.ts';
import type { LayoutServerLoad } from './$types';

/** Only the flag: the project itself is loaded (and authorized) by the `getProject` query. */
export const load: LayoutServerLoad = () => ({ chatEnabled: chatEnabled() });

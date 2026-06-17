import type { Handle } from '@sveltejs/kit';
import { getSession } from '$lib/session';
import { initKeys } from '$lib/jwt';
import { migrate } from '$lib/db';

let initialized = false;
async function init() {
	if (initialized) return;
	initialized = true;
	await migrate();
	await initKeys();
}

export const handle: Handle = async ({ event, resolve }) => {
	await init();
	event.locals.session = await getSession(event);
	return resolve(event);
};

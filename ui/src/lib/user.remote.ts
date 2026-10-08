import { getRequestEvent, query } from '$app/server';

/** The signed-in user (the hook guarantees one for every remote call). */
export const currentUser = query(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) throw new Error('Not signed in');
	return { login: user.login, name: user.name, avatarUrl: user.avatarUrl };
});

import { redirect } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { config } from '$lib/config';

export const GET: RequestHandler = async ({ url }) => {
	const returnTo = url.searchParams.get('returnTo') ?? '/';
	const state = encodeURIComponent(returnTo);
	const params = new URLSearchParams({
		response_type: 'code',
		client_id: config.dexClientId,
		redirect_uri: `https://${config.domain}/auth/callback`,
		scope: 'openid email profile',
		state
	});
	redirect(302, `${config.dexIssuer}/auth?${params}`);
};

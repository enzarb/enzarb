import { redirect, error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { config } from '$lib/config';

export const GET: RequestHandler = async ({ url, cookies }) => {
	if (!config.githubOAuthClientId) error(404, 'GitHub OAuth not configured');

	const returnTo = url.searchParams.get('returnTo') ?? '/';
	const state = crypto.randomUUID();

	// Store CSRF state and returnTo in a short-lived cookie (no session yet).
	cookies.set('github_login_state', JSON.stringify({ state, returnTo }), {
		path: '/auth/github/callback',
		httpOnly: true,
		secure: true,
		sameSite: 'lax',
		maxAge: 60 * 10 // 10 minutes
	});

	const params = new URLSearchParams({
		client_id: config.githubOAuthClientId,
		redirect_uri: `https://${config.domain}/auth/github/callback`,
		scope: 'repo read:user user:email',
		state
	});
	redirect(302, `https://github.com/login/oauth/authorize?${params}`);
};

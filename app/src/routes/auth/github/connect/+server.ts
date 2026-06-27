import { redirect, error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { config } from '$lib/config';
import { sql } from '$lib/db';
import { encrypt } from '$lib/crypto';

export const GET: RequestHandler = async ({ locals }) => {
	if (!config.githubOAuthClientId) error(404, 'GitHub OAuth not configured');
	if (!locals.session) redirect(302, '/login');

	// Generate and persist CSRF state — validated in the callback.
	const state = crypto.randomUUID();
	await sql`
		INSERT INTO user_secrets (user_id, key, value)
		VALUES (${locals.session.userId}, '_github_oauth_state', ${encrypt(state)})
		ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value
	`;

	const params = new URLSearchParams({
		client_id: config.githubOAuthClientId,
		redirect_uri: `https://${config.domain}/auth/github/callback`,
		scope: 'repo read:user user:email',
		state
	});
	redirect(302, `https://github.com/login/oauth/authorize?${params}`);
};

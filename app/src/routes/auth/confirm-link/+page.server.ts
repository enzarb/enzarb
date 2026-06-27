import type { PageServerLoad } from './$types';
import { config } from '$lib/config';
import { sql } from '$lib/db';

export const load: PageServerLoad = async ({ url }) => {
	const token = url.searchParams.get('token') ?? '';
	let valid = false;
	let githubEmail = '';
	if (token) {
		const rows = await sql<{ github_email: string }[]>`
			SELECT github_email FROM pending_account_links WHERE token = ${token} AND expires_at > now()
		`;
		valid = rows.length > 0;
		githubEmail = rows[0]?.github_email ?? '';
	}
	const state = encodeURIComponent(JSON.stringify({ pendingLinkToken: token, returnTo: '/settings?github=connected' }));
	const params = new URLSearchParams({
		response_type: 'code',
		client_id: config.dexClientId,
		redirect_uri: `https://${config.domain}/auth/callback`,
		scope: 'openid email profile',
		state
	});
	const oidcUrl = `${config.dexIssuer}/auth?${params}`;
	return { valid, githubEmail, oidcUrl, token };
};

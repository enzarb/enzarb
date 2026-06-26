import { redirect, error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { config } from '$lib/config';
import { sql } from '$lib/db';
import { orgNamespace, createOrPatchSecret, listProjects } from '$lib/k8s';

export const GET: RequestHandler = async ({ url, locals }) => {
	if (!config.githubOAuthClientId) error(404, 'GitHub OAuth not configured');
	if (!locals.session) redirect(302, '/login');

	const code = url.searchParams.get('code');
	const state = url.searchParams.get('state');
	if (!code) error(400, 'Missing code');

	// Validate CSRF state
	const stored = await sql<{ value: string }[]>`
		SELECT value FROM user_secrets
		WHERE user_id = ${locals.session.userId} AND key = '_github_oauth_state'
	`;
	if (!stored[0] || stored[0].value !== state) error(400, 'Invalid OAuth state');
	await sql`DELETE FROM user_secrets WHERE user_id = ${locals.session.userId} AND key = '_github_oauth_state'`;

	// Exchange code for access token
	const tokenRes = await fetch('https://github.com/login/oauth/access_token', {
		method: 'POST',
		headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
		body: JSON.stringify({
			client_id: config.githubOAuthClientId,
			client_secret: config.githubOAuthClientSecret,
			code
		})
	});
	if (!tokenRes.ok) error(502, 'GitHub token exchange failed');
	const tokenData = await tokenRes.json();
	const accessToken: string = tokenData.access_token;
	if (!accessToken) error(502, 'No access token returned from GitHub');

	// Fetch GitHub user profile
	const ghAuth = { Authorization: `Bearer ${accessToken}` };
	const [profileRes, emailsRes] = await Promise.all([
		fetch('https://api.github.com/user', { headers: ghAuth }),
		fetch('https://api.github.com/user/emails', { headers: ghAuth })
	]);
	if (!profileRes.ok) error(502, 'Failed to fetch GitHub profile');
	const profile = await profileRes.json();
	const emails: { email: string; primary: boolean; verified: boolean }[] = emailsRes.ok
		? await emailsRes.json()
		: [];

	const primaryEmail =
		emails.find(e => e.primary && e.verified)?.email ??
		emails.find(e => e.verified)?.email ??
		profile.email ??
		'';
	const displayName: string = profile.name ?? profile.login ?? '';

	// Upsert the four GitHub-related user secrets.
	const userId = locals.session.userId;
	const githubSecrets: Record<string, string> = {
		GH_TOKEN: accessToken,
		GITHUB_TOKEN: accessToken,
		...(displayName ? { ENZARB_GIT_USER_NAME: displayName } : {}),
		...(primaryEmail ? { ENZARB_GIT_USER_EMAIL: primaryEmail } : {})
	};
	for (const [key, value] of Object.entries(githubSecrets)) {
		await sql`
			INSERT INTO user_secrets (user_id, key, value) VALUES (${userId}, ${key}, ${value})
			ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value
		`;
	}

	// Sync all user secrets to K8s for every org the user belongs to.
	const allSecretRows = await sql<{ key: string; value: string }[]>`
		SELECT key, value FROM user_secrets WHERE user_id = ${userId}
			AND key != '_github_oauth_state'
	`;
	const allSecrets = Object.fromEntries(allSecretRows.map(r => [r.key, r.value]));
	for (const org of locals.session.orgs) {
		const ns = orgNamespace(org.id);
		await createOrPatchSecret(ns, `${org.id}-user-env-secrets`, allSecrets);
	}

	redirect(302, '/settings?github=connected');
};

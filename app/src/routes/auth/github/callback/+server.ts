import { redirect, error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { randomBytes } from 'crypto';
import { config } from '$lib/config';
import { sql } from '$lib/db';
import { encrypt, decrypt } from '$lib/crypto';
import { orgNamespace, createOrPatchSecret } from '$lib/k8s';
import { upsertGithubUser, createSession } from '$lib/session';

export const GET: RequestHandler = async ({ url, locals, cookies }) => {
	if (!config.githubOAuthClientId) error(404, 'GitHub OAuth not configured');

	const code = url.searchParams.get('code');
	const state = url.searchParams.get('state');
	if (!code) error(400, 'Missing code');

	const isConnect = !!locals.session;
	let returnTo = '/';

	if (isConnect) {
		// Connect flow: user is logged in; validate CSRF state from DB.
		const stored = await sql<{ value: string }[]>`
			SELECT value FROM user_secrets
			WHERE user_id = ${locals.session!.userId} AND key = '_github_oauth_state'
		`;
		if (!stored[0] || decrypt(stored[0].value) !== state) error(400, 'Invalid OAuth state');
		await sql`DELETE FROM user_secrets WHERE user_id = ${locals.session!.userId} AND key = '_github_oauth_state'`;
	} else {
		// Login flow: validate CSRF state from cookie.
		const raw = cookies.get('github_login_state');
		if (!raw) error(400, 'Missing login state');
		cookies.delete('github_login_state', { path: '/auth/github/callback' });
		let parsed: { state: string; returnTo: string };
		try { parsed = JSON.parse(raw); } catch { error(400, 'Invalid login state'); }
		if (parsed.state !== state) error(400, 'Invalid OAuth state');
		returnTo = parsed.returnTo ?? '/';
	}

	// Exchange code for access token.
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

	// Fetch GitHub user profile.
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
	const githubId: string = String(profile.id);

	// Resolve user ID.
	let userId: string;
	if (isConnect) {
		userId = locals.session!.userId;
		await sql`UPDATE users SET github_id = ${githubId} WHERE id = ${userId} AND github_id IS NULL`;
	} else {
		const result = await upsertGithubUser(githubId, primaryEmail, displayName);
		if (result.type === 'conflict') {
			// Store a pending link token so the user can confirm via OIDC re-auth.
			const linkToken = randomBytes(32).toString('hex');
			await sql`
				INSERT INTO pending_account_links
					(github_id, github_email, github_display_name, github_access_token, existing_user_id, token)
				VALUES (${githubId}, ${primaryEmail}, ${displayName}, ${encrypt(accessToken)}, ${result.existingUserId}, ${linkToken})
			`;
			redirect(302, `/auth/confirm-link?token=${linkToken}`);
		}
		userId = result.userId;
	}

	await syncGithubSecrets(userId, accessToken, displayName, primaryEmail);

	if (isConnect) {
		redirect(302, '/settings?github=connected');
	}

	// Login flow: create session and redirect.
	const sessionId = await createSession(userId);
	cookies.set('session', sessionId, {
		path: '/',
		httpOnly: true,
		secure: true,
		sameSite: 'lax',
		maxAge: 60 * 60 * 24 * 7
	});

	// New users with no username go through onboarding.
	const userRows = await sql<{ username: string | null }[]>`SELECT username FROM users WHERE id = ${userId}`;
	if (!userRows[0]?.username) {
		const dest = encodeURIComponent(returnTo);
		redirect(302, `/onboarding?returnTo=${dest}`);
	}

	const safe = returnTo.startsWith('/') && !returnTo.startsWith('//') ? returnTo : '/';
	redirect(302, safe);
};

async function syncGithubSecrets(userId: string, accessToken: string, displayName: string, primaryEmail: string) {
	const githubSecrets: Record<string, string> = {
		GH_TOKEN: accessToken,
		GITHUB_TOKEN: accessToken,
		...(displayName ? { ENZARB_GIT_USER_NAME: displayName } : {}),
		...(primaryEmail ? { ENZARB_GIT_USER_EMAIL: primaryEmail } : {})
	};
	for (const [key, value] of Object.entries(githubSecrets)) {
		await sql`
			INSERT INTO user_secrets (user_id, key, value) VALUES (${userId}, ${key}, ${encrypt(value)})
			ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value
		`;
	}

	// Sync all user secrets to K8s for every org the user belongs to.
	const allSecretRows = await sql<{ key: string; value: string }[]>`
		SELECT key, value FROM user_secrets
		WHERE user_id = ${userId} AND key NOT LIKE '\_%'
	`;
	const allSecrets = Object.fromEntries(allSecretRows.map(r => [r.key, decrypt(r.value)]));
	const orgs = await sql<{ id: string }[]>`
		SELECT o.id FROM organizations o
		JOIN org_members om ON om.org_id = o.id
		WHERE om.user_id = ${userId} AND o.deleted_at IS NULL
	`;
	for (const org of orgs) {
		await createOrPatchSecret(orgNamespace(org.id), `${org.id}-user-env-secrets`, allSecrets);
	}
}

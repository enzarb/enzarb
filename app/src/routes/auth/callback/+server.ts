import { redirect, error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import { config } from '$lib/config';
import { upsertUser, createSession } from '$lib/session';
import { sql } from '$lib/db';

export const GET: RequestHandler = async ({ url, cookies }) => {
	const code = url.searchParams.get('code');
	const state = url.searchParams.get('state') ?? '/';
	if (!code) error(400, 'Missing code');

	// Exchange code for tokens
	const tokenRes = await fetch(`${config.dexIssuer}/token`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
		body: new URLSearchParams({
			grant_type: 'authorization_code',
			code,
			redirect_uri: `https://${config.domain}/auth/callback`,
			client_id: config.dexClientId,
			client_secret: config.dexClientSecret
		})
	});
	if (!tokenRes.ok) error(500, 'Token exchange failed');
	const tokens = await tokenRes.json();

	// Get userinfo
	const userinfoRes = await fetch(`${config.dexIssuer}/userinfo`, {
		headers: { Authorization: `Bearer ${tokens.access_token}` }
	});
	if (!userinfoRes.ok) error(500, 'Userinfo failed');
	const userinfo = await userinfoRes.json();

	const userId = await upsertUser(userinfo.sub, userinfo.email);
	const sessionId = await createSession(userId);

	cookies.set('session', sessionId, {
		path: '/',
		httpOnly: true,
		secure: true,
		sameSite: 'lax',
		maxAge: 60 * 60 * 24 * 7
	});

	// New users (no username set) go through onboarding
	const userRows = await sql`SELECT username FROM users WHERE id = ${userId}`;
	const hasUsername = userRows[0]?.username != null;
	if (!hasUsername) {
		const returnTo = encodeURIComponent(state);
		redirect(302, `/onboarding?returnTo=${returnTo}`);
	}

	const destination = decodeURIComponent(state);
	const safe = destination.startsWith('/') && !destination.startsWith('//') ? destination : '/';
	redirect(302, safe);
};

import { form } from '$app/server';
import { getRequestEvent } from '$app/server';
import { error, redirect } from '@sveltejs/kit';
import { z } from 'zod/v4';
import { sql } from '$lib/db';
import { createOrgWithAdmin, isUsernameAvailable } from '$lib/orgs';

const USERNAME_RE = /^[a-z0-9][a-z0-9-]{1,37}[a-z0-9]$/;

export const chooseUsername = form(
	z.object({
		username: z
			.string()
			.min(3)
			.max(39)
			.toLowerCase()
			.regex(USERNAME_RE, 'Lowercase letters, numbers, and hyphens only; cannot start or end with a hyphen'),
		returnTo: z.string().default('/dashboard')
	}),
	async ({ username, returnTo }) => {
		const { locals } = getRequestEvent();
		if (!locals.session) error(401, 'Unauthorized');

		const available = await isUsernameAvailable(username);
		if (!available) error(422, 'That username is already taken.');

		await sql`UPDATE users SET username = ${username} WHERE id = ${locals.session.userId}`;
		await createOrgWithAdmin(username, username, locals.session.userId);

		const safe = returnTo.startsWith('/') && !returnTo.startsWith('//') ? returnTo : '/dashboard';
		redirect(303, safe);
	}
);

import { error } from '@sveltejs/kit';
import { getRequestEvent, query } from '$app/server';
import { getSql } from '#lib/server/db.ts';
import { getClaudeCredential } from '#lib/server/claude-credential.ts';

/** The signed-in user's Claude token presence (never the token). */
export const claudeCredential = query(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	return getClaudeCredential(getSql(), user.login);
});

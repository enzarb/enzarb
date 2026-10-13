import { error } from '@sveltejs/kit';
import { command, getRequestEvent, query } from '$app/server';
import { getSql } from '#lib/server/db.ts';
import { deleteOwnClaudeCredential, getClaudeCredential } from '#lib/server/claude-credential.ts';

/** The signed-in user's Claude token presence (never the token). */
export const claudeCredential = query(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	return getClaudeCredential(getSql(), user.login);
});

/** Removes the signed-in user's own Claude token. Takes no input: the owner is always the session login. */
export const removeClaudeCredential = command(async () => {
	const { user } = getRequestEvent().locals;
	if (!user) error(401, 'Not signed in');
	await deleteOwnClaudeCredential(getSql(), user.login);
});

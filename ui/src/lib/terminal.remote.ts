import { command } from '$app/server';
import { getConfig } from '#lib/server/config.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { mintSessionToken, muxUrl } from '#lib/server/session-jwt.ts';

/**
 * Mints a fresh short-lived proxy token and the websocket URL for it (contracts §7). Only the
 * signed-in owner of the Project's sandbox gets one (404 otherwise).
 */
export const terminalConnection = command(NameArg, async (name) => {
	const { user } = await ownedProject(name);
	const config = getConfig();
	const token = await mintSessionToken(config.sessionJwtSecret, user.login, name);
	return { url: muxUrl(config.publicUrl, name, token) };
});

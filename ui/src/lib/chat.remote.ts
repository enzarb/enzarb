import { command } from '$app/server';
import { getConfig } from '#lib/server/config.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { mintSessionToken, muxUrl } from '#lib/server/session-jwt.ts';

/** The chat's proxy token and websocket URL; minted only after the owner check (contracts §7). */
export const chatConnection = command(NameArg, async (name) => {
	const { user } = await ownedProject(name);
	const config = getConfig();
	const token = await mintSessionToken(config.sessionJwtSecret, user.login, name);
	return { url: muxUrl(config.publicUrl, name, token) };
});

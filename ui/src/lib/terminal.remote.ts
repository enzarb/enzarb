import { command, query } from '$app/server';
import * as v from 'valibot';
import { getConfig } from '#lib/server/config.ts';
import { getSql } from '#lib/server/db.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { projectSettings } from '#lib/server/project-settings.ts';
import { mintSessionToken, muxUrl } from '#lib/server/session-jwt.ts';

/** Mints a fresh short-lived proxy token and the websocket URL for it (contracts §7). */
export const terminalConnection = command(NameArg, async (name) => {
	const { user } = await ownedProject(name);
	const config = getConfig();
	const token = await mintSessionToken(config.sessionJwtSecret, user.login, name);
	return { url: muxUrl(config.publicUrl, name, token) };
});

export const getProjectSettings = query(NameArg, async (name) => {
	await ownedProject(name);
	return projectSettings(getSql()).get(name);
});

/** Called by the guided setup once the sandbox reports `credential_captured`. */
export const markClaudeConfigured = command(NameArg, async (name) => {
	const { user } = await ownedProject(name);
	await projectSettings(getSql()).markClaudeConfigured(name, user.login);
	await getProjectSettings(name).refresh();
});

const CredentialStateSchema = v.object({ name: NameArg, configured: v.boolean() });

/** Stores the sandbox's real `credential_state` answer as the cached `app.project_settings` flag. */
export const syncClaudeCredential = command(CredentialStateSchema, async ({ name, configured }) => {
	const { user } = await ownedProject(name);
	await projectSettings(getSql()).recordClaudeState(name, configured, user.login);
	await getProjectSettings(name).refresh();
});

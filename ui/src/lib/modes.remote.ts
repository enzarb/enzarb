import { error } from '@sveltejs/kit';
import { command, query } from '$app/server';
import * as v from 'valibot';
import { ModesDocSchema, RepoArg, SaveModesSchema, Str, TEXT_MAX } from '#lib/modes-schema.ts';
import type { ModeIssue, ModesDoc } from '#lib/modes-types.ts';
import { invalidateToken, token } from '#lib/server/github-app.ts';
import { loadModes, saveModes, type ModesDeps } from '#lib/server/modes-service.ts';
import { NameArg, ownedProject } from '#lib/server/owned-project.ts';
import { getRepositoryAccess } from '#lib/server/repo-access.ts';
import { getTracks, TracksError } from '#lib/server/tracks.ts';

/** Remote functions of the modes designer (Project → Modes, ADR-0029 §7). */

const UNAVAILABLE = {
	ok: false as const,
	code: 'unavailable' as const,
	message: 'The modes designer needs enzarb-tracks, which is not enabled on this installation.'
};

function deps(): ModesDeps | null {
	const tracks = getTracks();
	if (!tracks) return null;
	const access = getRepositoryAccess();
	return {
		tracks,
		checkAccess: (login, key) => access.check(login, key),
		token,
		invalidate: invalidateToken
	};
}

/** The `.ez/modes.toml` of one of the Project's repositories, parsed for the form. */
export const getModes = query(
	v.object({ name: NameArg, repo: RepoArg }),
	async ({ name, repo }) => {
		const { user, project } = await ownedProject(name);
		const d = deps();
		if (!d) return UNAVAILABLE;
		return loadModes(d, user, project, repo);
	}
);

async function withTracks<T>(name: string, run: (d: ModesDeps) => Promise<T>) {
	await ownedProject(name);
	const d = deps();
	if (!d) return UNAVAILABLE;
	try {
		return await run(d);
	} catch (e) {
		if (e instanceof TracksError) {
			if (e.status === 422)
				return {
					ok: false as const,
					code: 'invalid' as const,
					message: e.message,
					issues: e.issues
				};
			return {
				ok: false as const,
				code: 'tracks' as const,
				message: 'The modes service (enzarb-tracks) is not available.'
			};
		}
		throw e;
	}
}

/** Validate the form's document (called as the user edits). */
export const validateModes = command(
	v.object({ name: NameArg, doc: ModesDocSchema }),
	({ name, doc }) =>
		withTracks(name, async (d) => ({
			ok: true as const,
			issues: await d.tracks.validateModes(doc as ModesDoc)
		}))
);

/** Raw TOML to the form's document (switching from the TOML tab to the form). */
export const parseModesText = command(
	v.object({ name: NameArg, text: Str(TEXT_MAX) }),
	({ name, text }) =>
		withTracks(name, async (d) => ({ ok: true as const, ...(await d.tracks.parseModes(text)) }))
);

/** The file the form's document would produce, edited onto `old` (switching to the TOML tab). */
export const renderModesText = command(
	v.object({ name: NameArg, doc: ModesDocSchema, old: v.nullable(Str(TEXT_MAX)) }),
	({ name, doc, old }) =>
		withTracks(name, async (d) => {
			const r = await d.tracks.renderModes(doc as ModesDoc, old);
			return { ok: true as const, text: r.text, issues: r.issues as ModeIssue[] };
		})
);

/** Commit to the default branch (write access re-checked, blob-sha concurrency). */
export const saveModesFile = command(SaveModesSchema, async ({ name, ...input }) => {
	const { user, project } = await ownedProject(name);
	const d = deps();
	if (!d) return UNAVAILABLE;
	if (!user.githubId) error(400, 'Your session has no GitHub id; sign in again');
	const r = await saveModes(d, user, project, {
		...input,
		doc: input.doc as ModesDoc | undefined
	});
	if (r.ok && !r.unchanged) await getModes({ name, repo: input.repo }).refresh();
	return r;
});
